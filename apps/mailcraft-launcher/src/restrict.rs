// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Restricting this process with Landlock, best effort across kernel versions, refusing when
//! nothing is enforced.

use std::path::PathBuf;

use landlock::{ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetError, RulesetStatus, path_beneath_rules};

/// The newest Landlock ABI this launcher asks for; an older kernel enforces what it knows.
const ABI_WANTED: ABI = ABI::V5;

fn existing(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|p| p.exists()).cloned().collect()
}

/// Restricts this process (and what it executes) to reading and executing beneath `read` and
/// using `write` fully. Returns whether the kernel enforces it.
pub(crate) fn restrict(read: &[PathBuf], write: &[PathBuf]) -> Result<bool, RulesetError> {
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(ABI_WANTED))?
        .create()?
        .add_rules(path_beneath_rules(existing(read), AccessFs::from_read(ABI_WANTED)))?
        .add_rules(path_beneath_rules(existing(write), AccessFs::from_all(ABI_WANTED)))?
        .restrict_self()?;
    Ok(status.ruleset != RulesetStatus::NotEnforced)
}
