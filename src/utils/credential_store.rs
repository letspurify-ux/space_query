use keyring::Entry;

const SERVICE_NAME: &str = "space_query";
const LEGACY_SERVICE_NAME: &str = "oracle_query_tool";

fn entry_for_service(service_name: &str, connection_name: &str) -> Result<Entry, keyring::Error> {
    Entry::new(service_name, connection_name)
}

fn entry_for(connection_name: &str) -> Result<Entry, keyring::Error> {
    entry_for_service(SERVICE_NAME, connection_name)
}

/// Store a password in the OS keyring for the given connection name.
pub fn store_password(connection_name: &str, password: &str) -> Result<(), String> {
    let entry = entry_for(connection_name).map_err(|e| format!("Keyring error: {}", e))?;
    entry
        .set_password(password)
        .map_err(|e| format!("Failed to store password in keyring: {}", e))?;

    // Best-effort cleanup of legacy credentials after successful write.
    if let Ok(legacy_entry) = entry_for_service(LEGACY_SERVICE_NAME, connection_name) {
        let _ = legacy_entry.delete_credential();
    }

    Ok(())
}

/// Retrieve a password from the OS keyring for the given connection name.
/// Returns Ok(None) if no credential is found (not an error).
pub fn get_password(connection_name: &str) -> Result<Option<String>, String> {
    let entry = entry_for(connection_name).map_err(|e| format!("Keyring error: {}", e))?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => {
            let legacy_entry = entry_for_service(LEGACY_SERVICE_NAME, connection_name)
                .map_err(|e| format!("Keyring error: {}", e))?;
            match legacy_entry.get_password() {
                Ok(password) => {
                    // Migrate to new service namespace on read.
                    let _ = entry.set_password(&password);
                    let _ = legacy_entry.delete_credential();
                    Ok(Some(password))
                }
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(format!("Failed to retrieve password from keyring: {}", e)),
            }
        }
        Err(e) => Err(format!("Failed to retrieve password from keyring: {}", e)),
    }
}

/// Delete a password from the OS keyring for the given connection name.
/// Silently succeeds if no credential exists.
pub fn delete_password(connection_name: &str) -> Result<(), String> {
    let entry = entry_for(connection_name).map_err(|e| format!("Keyring error: {}", e))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(format!("Failed to delete password from keyring: {}", e)),
    }

    // Legacy cleanup (best effort, but return error on unexpected failures).
    let legacy_entry = entry_for_service(LEGACY_SERVICE_NAME, connection_name)
        .map_err(|e| format!("Keyring error: {}", e))?;
    match legacy_entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete password from keyring: {}", e)),
    }
}
