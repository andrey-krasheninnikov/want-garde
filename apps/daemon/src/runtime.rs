use crate::{
    Bot, Reply, StatusSource,
    delivery::{Outcome, Store},
    host::LinuxHost,
    telegram::{FailureKind, Telegram, TransportError},
};
use garde_config::{Config, Result};
use garde_contracts::{Classification, Event, Id, Severity, WireVersion};
use garde_entrypoint::message;
use serde_json::Value;
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

pub struct Identity {
    pub id: String,
    pub username: String,
}
pub trait Transport: Send + Sync + 'static {
    fn identity(
        &self,
    ) -> impl Future<Output = std::result::Result<Identity, TransportError>> + Send;
    fn updates(
        &self,
        offset: i64,
    ) -> impl Future<Output = std::result::Result<Vec<Value>, TransportError>> + Send;
    fn send(
        &self,
        reply: &Reply,
    ) -> impl Future<Output = std::result::Result<(), TransportError>> + Send;
}

type Work = Box<dyn FnOnce(&mut Store) + Send>;
#[derive(Clone)]
pub struct Database(mpsc::Sender<Work>);
impl Database {
    pub async fn open(config: Config) -> Result<Self> {
        let (sender, mut receiver) = mpsc::channel::<Work>(32);
        let (ready, wait) = oneshot::channel();
        std::thread::Builder::new()
            .name("garde-storage".into())
            .spawn(move || {
                let mut store = match Store::open(
                    &config.state_dir,
                    config.server_id.as_str(),
                    config.owner().unwrap_or(""),
                ) {
                    Ok(store) => store,
                    Err(code) => {
                        let _ = ready.send(Err(code));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while let Some(operation) = receiver.blocking_recv() {
                    operation(&mut store);
                }
            })
            .map_err(|_| "storage.worker")?;
        wait.await.map_err(|_| "storage.worker")??;
        Ok(Self(sender))
    }

    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = oneshot::channel();
        self.0
            .send(Box::new(move |store| {
                let _ = sender.send(operation(store));
            }))
            .await
            .map_err(|_| "storage.worker")?;
        receiver.await.map_err(|_| "storage.worker")?
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().min(i64::MAX as u64 - 86400) as i64)
        .unwrap_or(0)
}

pub async fn send_loop<T: Transport>(database: Database, transport: Arc<T>) -> Result<()> {
    let mut tasks: JoinSet<(i64, std::result::Result<(), TransportError>)> = JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_storage_error = None;
    loop {
        tokio::select! {
            _=tick.tick()=>{},
            result=tasks.join_next(),if !tasks.is_empty()=>{
                let (id,result)=result.ok_or("delivery.worker")?.map_err(|_|"delivery.worker")?;
                let fatal=result.as_ref().err().filter(|e|e.kind==FailureKind::Fatal).map(|e|e.code);
                match &result {Ok(())=>eprintln!("delivery.sent: d-{id}"),Err(error)=>eprintln!("{}: d-{id}",error.code)}
                let outcome=match result {
                    Ok(())=>Outcome::Delivered,
                    Err(error) if error.kind==FailureKind::Recipient=>Outcome::Permanent {code:error.code},
                    Err(error)=>Outcome::Retry {code:error.code,retry_after:error.retry_after.as_secs(),uncertain:error.uncertain},
                };
                loop {
                    match database.call(move|store|store.finish(id,outcome,now())).await {
                        Ok(())=>{last_storage_error=None;break;},
                        Err(code)=>{
                            if last_storage_error!=Some(code) {eprintln!("{code}");last_storage_error=Some(code);}
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
                if let Some(code)=fatal { return Err(code); }
            }
        }
        while tasks.len() < 4 {
            let next = match database.call(|store| store.claim(now())).await {
                Ok(next) => {
                    last_storage_error = None;
                    next
                }
                Err(code) => {
                    if last_storage_error != Some(code) {
                        eprintln!("{code}");
                        last_storage_error = Some(code);
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    break;
                }
            };
            let Some(delivery) = next else {
                break;
            };
            let transport = transport.clone();
            tasks.spawn(async move { (delivery.id, transport.send(&delivery.reply).await) });
        }
    }
}

async fn poll_loop<T: Transport>(
    mut bot: Bot,
    database: Database,
    transport: Arc<T>,
    source: &impl StatusSource,
    username: &str,
    mut startup: Option<Event>,
) -> Result<()> {
    let mut delay = 1u64;
    let mut last_error = None;
    loop {
        if let Some(event) = startup.take()
            && let Err(code) = bot.publish_event(&database, event.clone()).await
        {
            if last_error != Some(code) {
                eprintln!("{code}");
                last_error = Some(code);
            }
            startup = Some(event);
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
            continue;
        }
        let offset = match database.call(|store| store.poll_offset(now())).await {
            Ok(offset) => offset,
            Err(code) => {
                if last_error != Some(code) {
                    eprintln!("{code}");
                    last_error = Some(code);
                }
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
                continue;
            }
        };
        let mut updates = match transport.updates(offset).await {
            Ok(updates) => updates,
            Err(error) if error.kind == FailureKind::Fatal => return Err(error.code),
            Err(error) => {
                if last_error != Some(error.code) {
                    eprintln!("{}", error.code);
                    last_error = Some(error.code);
                }
                tokio::time::sleep(Duration::from_secs(delay).max(error.retry_after)).await;
                delay = (delay * 2).min(30);
                continue;
            }
        };
        updates.sort_by_key(|update| update["update_id"].as_i64().unwrap_or(-1));
        let mut failed = false;
        for update in updates {
            if let Err(code) = bot
                .process_update(&database, &update, source, username)
                .await
            {
                if last_error != Some(code) {
                    eprintln!("{code}");
                    last_error = Some(code);
                }
                failed = true;
                break;
            }
        }
        if failed {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        } else {
            delay = 1;
            last_error = None;
        }
    }
}

pub async fn serve(config: Config) -> Result<()> {
    let transport = Arc::new(Telegram::new(config.token()?).map_err(|error| error.code)?);
    let database = Database::open(config.clone()).await?;
    let bot = Bot::new(config.clone())?;
    let id = database.call(|store| store.start_id()).await?;
    let startup = Event {
        version: WireVersion::V1,
        id,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(now(), 0).ok_or("service.clock")?,
        source: Id::new("service")?,
        classification: Classification {
            category: Id::new("service.lifecycle")?,
            severity: Severity::Info,
            rule_id: Id::new("service.started")?,
            reason_key: Id::new("service_started")?,
        },
        evidence: vec![],
    };
    let startup = match bot.publish_event(&database, startup.clone()).await {
        Ok(()) => None,
        Err(code) => {
            eprintln!("{code}");
            Some(startup)
        }
    };
    let source = LinuxHost::with_sources(
        crate::host::HostSources::default(),
        config.host_health.disk_paths.clone(),
    );
    let mut host = crate::host_health::Monitor::new(config.clone(), database.clone());
    let mut services = crate::services::Monitor::new(config.clone(), database.clone());
    let mut ports = crate::listeners::Monitor::new(config.clone(), database.clone());
    let mut xui = crate::xui::Monitor::new(config.clone(), database.clone());
    let mut collector = crate::ssh::Collector::new(config.clone(), database.clone());
    let mut fail2ban = crate::ingest::Collector::for_source(
        crate::ingest::Kind::Fail2ban,
        config.clone(),
        database.clone(),
    );
    let mut firewall = crate::ingest::Collector::for_source(
        crate::ingest::Kind::Firewall,
        config.clone(),
        database.clone(),
    );
    tokio::try_join!(
        host.run(&source),
        services.run(),
        ports.run(),
        xui.run(),
        collector.run(),
        fail2ban.run(),
        firewall.run(),
        async {
            let mut delay = 1u64;
            let identity = loop {
                match transport.identity().await {
                    Ok(identity) => break identity,
                    Err(error) if error.kind == FailureKind::Fatal => return Err(error.code),
                    Err(error) => {
                        eprintln!(
                            "{}: {}",
                            error.code,
                            message(&config.default_locale, "operation_failed")
                        );
                        tokio::time::sleep(Duration::from_secs(delay).max(error.retry_after)).await;
                        delay = (delay * 2).min(30);
                    }
                }
            };
            database
                .call(move |store| store.bind_bot(&identity.id))
                .await?;
            eprintln!(
                "service.ready: {}",
                message(&config.default_locale, "service_ready")
            );
            tokio::try_join!(
                poll_loop(
                    bot,
                    database.clone(),
                    transport.clone(),
                    &source,
                    &identity.username,
                    startup
                ),
                send_loop(database, transport)
            )?;
            Ok(())
        }
    )?;
    Ok(())
}
