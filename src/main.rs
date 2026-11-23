mod api;
mod auth;
mod gateway;
mod model;
mod network;
mod notifications;
mod qr_auth;
mod search;
mod store;
mod typing;
mod ui;
mod voice;

pub fn ensure_tls_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn main() {
    ensure_tls_provider();
    ui::run();
}
