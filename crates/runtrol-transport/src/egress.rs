//! Default-deny socket egress for phone transports.
//!
//! Callers cannot pass a hostname to this module. Discovery resolves and validates a destination first, then an
//! immutable policy approves one exact IP address and port. The actual operating-system dial exists only here.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};

use tokio::net::TcpStream;

/// Exact destinations a phone transport may dial.
#[derive(Clone, Debug, Default)]
pub struct EgressPolicy {
    allowed: BTreeSet<SocketAddr>,
}

impl EgressPolicy {
    /// Build an immutable exact-address allowlist.
    ///
    /// An empty iterator is the safe first-run state and denies every destination.
    #[must_use]
    pub fn new(addresses: impl IntoIterator<Item = SocketAddr>) -> Self {
        Self {
            allowed: addresses.into_iter().collect(),
        }
    }

    /// Mint a destination capability only for an exact allowed IP address and port.
    ///
    /// # Errors
    ///
    /// [`EgressError::Refused`] when `address` is absent from the allowlist.
    pub fn approve(&self, address: SocketAddr) -> Result<ApprovedDestination, EgressError> {
        if self.allowed.contains(&address) {
            Ok(ApprovedDestination { address })
        } else {
            Err(EgressError::Refused { address })
        }
    }

    /// Open a TCP stream to an approved destination.
    ///
    /// The address is checked again so a capability minted by a different policy cannot cross policy boundaries.
    ///
    /// # Errors
    ///
    /// [`EgressError::Refused`] if this policy does not contain the destination, or [`EgressError::Connect`] when
    /// the operating system refuses the connection.
    pub async fn connect(
        &self,
        destination: ApprovedDestination,
    ) -> Result<TcpStream, EgressError> {
        let address = destination.address;
        if !self.allowed.contains(&address) {
            return Err(EgressError::Refused { address });
        }
        TcpStream::connect(address)
            .await
            .map_err(|source| EgressError::Connect { address, source })
    }
}

/// A capability for one exact socket destination.
///
/// Its fields are private. Only [`EgressPolicy::approve`] can construct one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovedDestination {
    address: SocketAddr,
}

/// Whether a resolved address stays outside local, private, link-local, documentation, and unspecified ranges.
///
/// DNS-derived phone egress uses this one predicate before exact socket capabilities are minted. TLS still
/// authenticates the DNS name after the approved address is connected.
pub(crate) fn public_internet_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_documentation()
                && !address.is_unspecified()
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
        }
    }
}

/// A default-deny egress decision or connection failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EgressError {
    /// The exact IP address and port were not allowlisted.
    #[error("outbound destination {address} is not allowlisted")]
    Refused {
        /// The refused destination.
        address: SocketAddr,
    },

    /// The operating system could not connect to an allowed destination.
    #[error("could not connect to allowed destination {address}: {source}")]
    Connect {
        /// The approved destination.
        address: SocketAddr,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },
}
