//! Thin zbus client proxies for the frozen `org.apexos.Apexd1` surface, plus
//! helpers for connecting and probing whether the daemon is actually running.

use zbus::proxy;

pub const BUS_NAME: &str = "org.apexos.Apexd1";

#[proxy(
    interface = "org.apexos.Apexd1.Power",
    default_service = "org.apexos.Apexd1",
    default_path = "/org/apexos/Apexd1"
)]
pub trait Power {
    fn set_tier(&self, tier: &str) -> zbus::Result<()>;
    fn set_auto_switch(&self, enabled: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn tier(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn tiers(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn on_ac_power(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn auto_switch(&self) -> zbus::Result<bool>;
}

#[proxy(
    interface = "org.apexos.Apexd1.Battery",
    default_service = "org.apexos.Apexd1",
    default_path = "/org/apexos/Apexd1"
)]
pub trait Battery {
    fn set_charge_thresholds(&self, start: u8, end: u8) -> zbus::Result<()>;
    fn set_travel_mode(&self, enabled: bool) -> zbus::Result<()>;
    fn calibrate(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn charge_start(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn charge_end(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn travel_mode(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn capacity(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;
}

#[proxy(
    interface = "org.apexos.Apexd1.Profile",
    default_service = "org.apexos.Apexd1",
    default_path = "/org/apexos/Apexd1"
)]
pub trait Profile {
    #[zbus(property)]
    fn active(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn class(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn device(&self) -> zbus::Result<String>;
}

/// Connect to the system bus, returning None (never panicking) if the bus
/// itself is unreachable.
pub async fn connect() -> Option<zbus::Connection> {
    zbus::Connection::system().await.ok()
}

/// True when `apexd` currently owns its well-known name on the bus.
pub async fn daemon_running(conn: &zbus::Connection) -> bool {
    match zbus::fdo::DBusProxy::new(conn).await {
        Ok(dbus) => dbus
            .name_has_owner(BUS_NAME.try_into().unwrap())
            .await
            .unwrap_or(false),
        Err(_) => false,
    }
}
