//! Shared provider preparation below both daemon request boundaries.

use std::collections::BTreeSet;
use std::sync::Arc;

use runtrol_core::registry::KindStatus;
use runtrol_drivers::DriverContext;
use runtrol_provider::{Provider, ProviderId};

use crate::Composed;

/// A provider could not be prepared from registered runtime observations.
pub(crate) struct ProviderPreparationError {
    message: String,
}

impl ProviderPreparationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Safe local control-surface explanation.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

/// Build one declared and available driver, with runtime resolution owned by the probe.
pub(crate) async fn driver(
    composed: &Composed,
    id: ProviderId,
) -> Result<Box<dyn Provider>, ProviderPreparationError> {
    let provider = id.as_str();
    let Some(declared) = composed.registry.get(id) else {
        return Err(ProviderPreparationError::new(format!(
            "no provider called {provider}"
        )));
    };
    match declared.kind {
        KindStatus::Available => {}
        KindStatus::Unavailable { why } => return Err(ProviderPreparationError::new(why)),
        KindStatus::Unknown => {
            return Err(ProviderPreparationError::new(format!(
                "{provider} names a kind nothing in this build declares"
            )));
        }
    }

    let Some(entry) = composed.driver_for(declared.manifest.kind.as_str()) else {
        return Err(ProviderPreparationError::new(
            "this build has no driver for that kind",
        ));
    };
    let Some(make) = entry.make else {
        return Err(ProviderPreparationError::new(
            entry
                .unavailable
                .unwrap_or("this build cannot serve that kind"),
        ));
    };

    let mut cache = runtrol_core::ProbeCache::open(composed.home.paths().probe_cache());
    let bound_flags = entry.flags.iter().map(|flag| flag.flag).collect::<Vec<_>>();
    // Resolution belongs to the probe, and the returned value is the exact program handed to the driver. Resolving
    // again here would let a PATH change select a different executable from the one whose version and flags were read.
    let (program, probed) = runtrol_core::probe_program(
        &declared.manifest,
        &bound_flags,
        &mut cache,
        &composed.containment,
    )
    .await
    .map_err(|error| ProviderPreparationError::new(error.to_string()))?;
    cache
        .save()
        .map_err(|error| ProviderPreparationError::new(error.to_string()))?;

    let checked = checked_flags(provider, entry, probed.flags)?;

    Ok(make(&DriverContext {
        provider: id,
        models: declared.manifest.models.clone(),
        program,
        transport_argv: declared.manifest.transport.argv.clone(),
        available_flags: checked.available,
        unavailable_flags: checked.unavailable,
        contained_by: Arc::clone(&composed.containment),
    }))
}

/// Exact confirmed and unavailable optional flags for one prepared driver.
#[derive(Debug)]
pub(crate) struct CheckedFlags {
    pub(crate) available: BTreeSet<Box<str>>,
    pub(crate) unavailable: std::collections::BTreeMap<Box<str>, &'static str>,
}

/// Validate a driver's required and optional flags against the exact probed program.
pub(crate) fn checked_flags(
    provider: &str,
    driver: &runtrol_drivers::DriverKind,
    observed: runtrol_core::Flags,
) -> Result<CheckedFlags, ProviderPreparationError> {
    let available: BTreeSet<Box<str>> = match observed {
        runtrol_core::Flags::Observed(flags) => flags.into_iter().map(Into::into).collect(),
        runtrol_core::Flags::Unknown { why } if driver.flags.iter().any(|flag| flag.required) => {
            return Err(ProviderPreparationError::new(format!(
                "{provider} could not confirm the flags its driver requires: {why}"
            )));
        }
        runtrol_core::Flags::Unknown { .. } => BTreeSet::default(),
    };
    for required in driver.flags.iter().filter(|flag| flag.required) {
        if !available.contains(required.flag) {
            return Err(ProviderPreparationError::new(format!(
                "{provider} does not accept required flag {}: {}",
                required.flag, required.without_it
            )));
        }
    }
    let unavailable = driver
        .flags
        .iter()
        .filter(|flag| !flag.required && !available.contains(flag.flag))
        .map(|flag| (Box::<str>::from(flag.flag), flag.without_it))
        .collect();
    Ok(CheckedFlags {
        available,
        unavailable,
    })
}
