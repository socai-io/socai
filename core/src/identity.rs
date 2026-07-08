use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct IdentityFile {
    install_id: String,
}

pub fn load_or_create_install_id(home: &Path) -> String {
    let path = home.join("telemetry/identity.json");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(identity) = serde_json::from_slice::<IdentityFile>(&bytes) {
            let id = identity.install_id.trim();
            if Uuid::parse_str(id).is_ok() {
                return id.to_string();
            }
        }
    }

    let install_id = Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_vec_pretty(&IdentityFile {
            install_id: install_id.clone(),
        })
        .unwrap_or_else(|_| b"{}".to_vec()),
    );
    install_id
}
