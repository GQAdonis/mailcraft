// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How a model server's name becomes addresses (#ai, D-rebind).
//!
//! A plain-HTTP model server must be this machine. Its name is looked up **once**, through a
//! [`Resolve`], every address that one answer gives must be a loopback address, and the
//! connection is made to such an address itself (`http::connect_loopback`), so a second look-up
//! that would answer something else (DNS rebinding) is never made. In the program the lookup is
//! the system's ([`SystemResolver`]); a test hands in one of its own, to show that the answer
//! that was checked is the one connected to.

use std::net::{SocketAddr, ToSocketAddrs};

/// Most addresses of one answer that are kept: more is a hostile or broken resolver.
pub(crate) const MAX_RESOLVED: usize = 32;

/// Looks a name up. One call is one lookup.
pub(crate) trait Resolve {
    /// The addresses `host` stands for, with `port`. An error when the name can't be looked up.
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

/// The system's resolver: what every connection of the program uses.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok((host, port).to_socket_addrs()?.take(MAX_RESOLVED).collect())
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod resolve_tests;
