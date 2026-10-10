/** MCP HTTP 令牌在 keyring 里的账号名。json 里只保存这个引用。 */
pub fn authorization_reference(server_name: &str) -> String {
    format!("mcp-auth:{server_name}")
}

pub fn save_secret(reference: &str, secret: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(service(), reference)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;
    entry
        .set_password(secret)
        .map_err(|error| format!("无法保存 MCP 令牌：{error}"))?;
    Ok(())
}

pub fn load_secret(reference: &str) -> Option<String> {
    let entry = keyring::Entry::new(service(), reference).ok()?;
    entry.get_password().ok().filter(|secret| !secret.trim().is_empty())
}

pub fn delete_secret(reference: &str) {
    if let Ok(entry) = keyring::Entry::new(service(), reference) {
        let _ = entry.delete_credential();
    }
}

fn service() -> &'static str {
    crate::storage::keyring_service_for_build(cfg!(target_os = "macos"), cfg!(debug_assertions))
}
