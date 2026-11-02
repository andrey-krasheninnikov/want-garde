use crate::*;
use std::collections::BTreeMap;

pub fn contract_artifacts() -> Result<BTreeMap<&'static str, String>, serde_json::Error> {
    let mut files = BTreeMap::new();
    macro_rules! schema {
        ($name:literal, $kind:ty) => {
            files.insert(
                $name,
                serde_json::to_string_pretty(&schemars::schema_for!($kind))? + "\n",
            );
        };
    }
    schema!("operation-request.schema.json", OperationRequest);
    schema!("event.schema.json", Event);
    schema!("incident.schema.json", Incident);
    schema!("grant.schema.json", Grant);
    schema!("job.schema.json", Job);
    schema!("host-snapshot.schema.json", HostSnapshot);

    let mut types = format!(
        "export const PRODUCT_VERSION = {:?} as const;\nexport const CONTRACT_VERSION = \"1\" as const;\n",
        PRODUCT_VERSION
    );
    macro_rules! declaration {
        ($($kind:ty),+ $(,)?) => {
            $(types.push_str(&format!("export {}\n", <$kind as ts_rs::TS>::decl(&ts_rs::Config::default())));)+
        };
    }
    declaration!(
        Id,
        WireVersion,
        Action,
        OperationRequest,
        ErrorCode,
        ContractError,
        Severity,
        Classification,
        Evidence,
        Event,
        IncidentState,
        Incident,
        Capability,
        Scope,
        Grant,
        Measurement<String>,
        ByteUsage,
        HostSnapshot,
        OperationResult,
        JobState,
        Job,
    );
    files.insert("index.ts", types);
    Ok(files)
}
