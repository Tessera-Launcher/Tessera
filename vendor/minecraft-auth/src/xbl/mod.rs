mod error;
mod model;
mod request;
mod sign;

pub use model::{XblDeviceToken, XblUserToken, XblXstsToken};
pub use request::{device_authenticate, sisu_authorize, user_authenticate, xsts_authorize};

pub mod constants {
    pub const XBL_AUTH_RELYING_PARTY: &str = "http://auth.xboxlive.com";
    pub const JAVA_XSTS_RELYING_PARTY: &str = "rp://api.minecraftservices.com/";
}
