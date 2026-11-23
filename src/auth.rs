use zeroize::Zeroizing;
fn entry() -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new("rustcord", "discord-token")
}
pub fn load() -> Result<Option<Zeroizing<String>>, keyring::Error> {
    match entry()?.get_password() {
        Ok(token) => Ok(Some(Zeroizing::new(token))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}
pub fn save(token: &str) -> Result<(), keyring::Error> {
    entry()?.set_password(token)
}
pub fn clear() -> Result<(), keyring::Error> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}
