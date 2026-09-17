use windows_sys::Win32::NetworkManagement::IpHelper::GetNetworkConnectivityHint;
use windows_sys::Win32::Networking::WinSock::{
    NL_NETWORK_CONNECTIVITY_HINT, NetworkConnectivityLevelHintConstrainedInternetAccess as CONSTRAINED,
    NetworkConnectivityLevelHintInternetAccess as INTERNET, NetworkConnectivityLevelHintLocalAccess as LOCAL,
    NetworkConnectivityLevelHintNone as NONE,
};

use crate::watch::wire::Reach;

pub fn reach() -> Reach {
    let mut hint = NL_NETWORK_CONNECTIVITY_HINT::default();
    if unsafe { GetNetworkConnectivityHint(&mut hint) } != 0 {
        return Reach::Unknown;
    }
    match hint.ConnectivityLevel {
        INTERNET => Reach::Internet,
        LOCAL | CONSTRAINED => Reach::Local,
        NONE => Reach::Nothing,
        _ => Reach::Unknown,
    }
}
