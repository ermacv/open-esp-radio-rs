//! Small checked AF_PACKET owner shared by raw 802.11 HIL injectors.

use std::{
    ffi::CString,
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};

use crate::Result;

const ARPHRD_IEEE80211_RADIOTAP: u16 = 803;
const ETH_P_ALL: u16 = 0x0003;

pub(crate) fn ensure_monitor_interface(interface: &str) -> Result<()> {
    let kind = fs::read_to_string(Path::new("/sys/class/net").join(interface).join("type"))
        .map_err(|error| format!("cannot inspect interface `{interface}`: {error}"))?;
    let kind = kind.trim().parse::<u16>()?;
    if kind != ARPHRD_IEEE80211_RADIOTAP {
        return Err(format!(
            "interface `{interface}` has ARPHRD type {kind}, expected monitor/radiotap type \
             {ARPHRD_IEEE80211_RADIOTAP}"
        )
        .into());
    }
    Ok(())
}

pub(crate) struct PacketSocket {
    descriptor: OwnedFd,
    address: libc::sockaddr_ll,
}

impl PacketSocket {
    pub(crate) fn bind(interface: &str) -> Result<Self> {
        let interface = CString::new(interface)?;
        // SAFETY: `interface` is a live NUL-terminated C string for the
        // duration of the call.
        let interface_index = unsafe { libc::if_nametoindex(interface.as_ptr()) };
        if interface_index == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: `socket` has no pointer arguments. A nonnegative return
        // value transfers one owned descriptor to this scope.
        let raw_descriptor = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                i32::from(ETH_P_ALL.to_be()),
            )
        };
        if raw_descriptor < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: the successful `socket` call returned a fresh descriptor
        // that has not been wrapped or closed elsewhere.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw_descriptor) };
        let address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: ETH_P_ALL.to_be(),
            sll_ifindex: interface_index as i32,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            sll_addr: [0; 8],
        };
        Ok(Self {
            descriptor,
            address,
        })
    }

    pub(crate) fn send(&self, packet: &[u8]) -> Result<()> {
        // SAFETY: both pointers refer to live immutable objects for the call;
        // their byte lengths are exact, and the owned descriptor remains open.
        let sent = unsafe {
            libc::sendto(
                self.descriptor.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                0,
                (&raw const self.address).cast(),
                size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if sent as usize != packet.len() {
            return Err(format!("short monitor injection: {sent}/{}", packet.len()).into());
        }
        Ok(())
    }
}
