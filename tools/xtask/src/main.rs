use garde_contracts::{PRODUCT_VERSION, contract_artifacts};
use std::{env, ffi::OsString, fs, path::PathBuf, process::ExitCode};

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn contracts(args: &[OsString]) -> Result<(), &'static str> {
    let mut output = None;
    let mut check = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--check") if !check => check = true,
            Some("--output-dir") if output.is_none() => {
                index += 1;
                output = Some(PathBuf::from(args.get(index).ok_or("invalid_arguments")?));
            }
            _ => return Err("invalid_arguments"),
        }
        index += 1;
    }
    let output = output.unwrap_or_else(|| repository().join("packages/contracts/generated"));
    let artifacts = contract_artifacts().map_err(|_| "contract_export_failed")?;
    if !check {
        fs::create_dir_all(&output).map_err(|_| "output_unavailable")?;
    }
    for (name, content) in artifacts {
        let path = output.join(name);
        if check {
            if fs::read_to_string(path).ok().as_deref() != Some(&content) {
                return Err("contract_artifacts_out_of_date");
            }
        } else {
            fs::write(path, content).map_err(|_| "contract_export_failed")?;
        }
    }
    Ok(())
}

fn versions() -> Result<(), &'static str> {
    let root = repository();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("package.json")).map_err(|_| "manifest_unavailable")?,
    )
    .map_err(|_| "invalid_manifest")?;
    let workspaces = manifest["workspaces"]
        .as_array()
        .ok_or("invalid_manifest")?;
    if manifest["version"] != PRODUCT_VERSION {
        return Err("version_mismatch");
    }
    let lock: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("package-lock.json")).map_err(|_| "lockfile_unavailable")?,
    )
    .map_err(|_| "invalid_lockfile")?;
    if lock["version"] != PRODUCT_VERSION || lock["packages"][""]["version"] != PRODUCT_VERSION {
        return Err("lockfile_version_mismatch");
    }
    for workspace in workspaces {
        let name = workspace.as_str().ok_or("invalid_manifest")?;
        let package: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(name).join("package.json")).map_err(|_| "manifest_unavailable")?,
        )
        .map_err(|_| "invalid_manifest")?;
        if package["version"] != PRODUCT_VERSION
            || lock["packages"][name]["version"] != PRODUCT_VERSION
        {
            return Err("workspace_version_mismatch");
        }
    }
    Ok(())
}

fn run() -> Result<(), &'static str> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("contracts") => contracts(&args[1..]),
        Some("versions") if args.len() == 1 => versions(),
        Some("--help") if args.len() == 1 => {
            println!(
                "cargo xtask contracts [--check] [--output-dir DIRECTORY]\ncargo xtask versions"
            );
            Ok(())
        }
        _ => Err("invalid_arguments"),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("{code}");
            ExitCode::from(2)
        }
    }
}
