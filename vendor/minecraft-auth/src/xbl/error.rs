use reqwest::StatusCode;

use crate::error::Error;

pub(crate) fn xbl_error(status: StatusCode, code: u64, detail: &str) -> Error {
    let (name, message) = describe(code);
    let message = if detail.is_empty() {
        message.to_string()
    } else if message == "An unknown error occurred" {
        // For a code the table does not name, Xbox's own words are the only
        // thing we can hand to the user.
        detail.to_string()
    } else {
        format!("{message}（{detail}）")
    };
    Error::Xbl {
        status,
        error_code: code,
        name,
        message,
    }
}

/// Named Xbox Live/account error codes, sourced from
/// https://github.com/microsoft/xbox-live-api. Most carry no
/// service-provided message, so a handful of the most common ones are
/// annotated with human-readable guidance.
fn describe(code: u64) -> (String, &'static str) {
    let name = match code {
        0x87DD0003 => "AM_E_XASD_UNEXPECTED",
        0x87DD0004 => "AM_E_XASU_UNEXPECTED",
        0x87DD0005 => "AM_E_XAST_UNEXPECTED",
        0x87DD0006 => "AM_E_XSTS_UNEXPECTED",
        0x87DD0007 => "AM_E_XDEVICE_UNEXPECTED",
        0x87DD0008 => "AM_E_DEVMODE_NOT_AUTHORIZED",
        0x87DD0009 => "AM_E_NOT_AUTHORIZED",
        0x87DD000A => "AM_E_FORBIDDEN",
        0x87DD000B => "AM_E_UNKNOWN_TARGET",
        0x87DD000C => "AM_E_INVALID_NSAL_DATA",
        0x87DD000D => "AM_E_TITLE_NOT_AUTHENTICATED",
        0x87DD000E => "AM_E_TITLE_NOT_AUTHORIZED",
        0x87DD000F => "AM_E_DEVICE_NOT_AUTHENTICATED",
        0x87DD0010 => "AM_E_INVALID_USER_INDEX",

        0x8015DC00 => "XO_E_DEVMODE_NOT_AUTHORIZED",
        0x8015DC01 => "XO_E_SYSTEM_UPDATE_REQUIRED",
        0x8015DC02 => "XO_E_CONTENT_UPDATE_REQUIRED",
        0x8015DC03 => "XO_E_ENFORCEMENT_BAN",
        0x8015DC04 => "XO_E_THIRD_PARTY_BAN",
        0x8015DC05 => "XO_E_ACCOUNT_PARENTALLY_RESTRICTED",
        0x8015DC06 => "XO_E_DEVICE_SUBSCRIPTION_NOT_ACTIVATED",
        0x8015DC08 => "XO_E_ACCOUNT_BILLING_MAINTENANCE_REQUIRED",
        0x8015DC09 => "XO_E_ACCOUNT_CREATION_REQUIRED",
        0x8015DC0A => "XO_E_ACCOUNT_TERMS_OF_USE_NOT_ACCEPTED",
        0x8015DC0B => "XO_E_ACCOUNT_COUNTRY_NOT_AUTHORIZED",
        0x8015DC0C => "XO_E_ACCOUNT_AGE_VERIFICATION_REQUIRED",
        0x8015DC0D => "XO_E_ACCOUNT_CURFEW",
        0x8015DC0E => "XO_E_ACCOUNT_CHILD_NOT_IN_FAMILY",
        0x8015DC0F => "XO_E_ACCOUNT_CSV_TRANSITION_REQUIRED",
        0x8015DC10 => "XO_E_ACCOUNT_MAINTENANCE_REQUIRED",
        0x8015DC11 => "XO_E_ACCOUNT_TYPE_NOT_ALLOWED",
        0x8015DC12 => "XO_E_CONTENT_ISOLATION",
        0x8015DC13 => "XO_E_ACCOUNT_NAME_CHANGE_REQUIRED",
        0x8015DC14 => "XO_E_DEVICE_CHALLENGE_REQUIRED",
        0x8015DC16 => "XO_E_SIGNIN_COUNT_BY_DEVICE_TYPE_EXCEEDED",
        0x8015DC17 => "XO_E_PIN_CHALLENGE_REQUIRED",
        0x8015DC18 => "XO_E_RETAIL_ACCOUNT_NOT_ALLOWED",
        0x8015DC19 => "XO_E_SANDBOX_NOT_ALLOWED",
        0x8015DC1A => "XO_E_ACCOUNT_SERVICE_UNAVAILABLE_UNKNOWN_USER",
        0x8015DC1B => "XO_E_GREEN_SIGNED_CONTENT_NOT_AUTHORIZED",
        0x8015DC1C => "XO_E_CONTENT_NOT_AUTHORIZED",
        0x8015DC20 => "XO_E_EXPIRED_DEVICE_TOKEN",
        0x8015DC21 => "XO_E_EXPIRED_TITLE_TOKEN",
        0x8015DC22 => "XO_E_EXPIRED_USER_TOKEN",
        0x8015DC23 => "XO_E_INVALID_DEVICE_TOKEN",
        0x8015DC24 => "XO_E_INVALID_TITLE_TOKEN",
        0x8015DC25 => "XO_E_INVALID_USER_TOKEN",

        _ => return (format!("XERR_{code:#010X}"), "An unknown error occurred"),
    };

    let message = match code {
        0x8015DC03 => "Your account was banned by Xbox for violating one or more Community Standards for Xbox.",
        0x8015DC05 => "Your account is currently restricted and your guardian has not given you permission to play online. Login to https://account.microsoft.com/family/ and have your guardian change your permissions.",
        0x8015DC09 => "Your account doesn't have an Xbox profile. Please create one at https://www.xbox.com/live",
        0x8015DC0A => "Your account has not accepted Xbox's Terms of Service. Please login at https://www.xbox.com/live and accept them.",
        0x8015DC0B => "Your account is from a country where Xbox Live is not available/banned.",
        0x8015DC0C => "Your account requires proof of age. Please login to https://login.live.com/login.srf and provide proof of age.",
        0x8015DC0D => "Your account has reached its limit for playtime. Your account has been blocked from logging in.",
        0x8015DC0E => "Your account is a child (under 18) and cannot proceed unless the account is added to a Family by an adult.",
        _ => "An unknown error occurred",
    };

    (name.to_string(), message)
}
