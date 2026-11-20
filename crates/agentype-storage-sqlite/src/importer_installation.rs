//! M6-B.5 B5.2 trusted-importer installation epochs (schema v12).
//!
//! A profile digest and an attestation value can both be reconstructed from
//! public parts. Neither can prove *which* physical installation observed a
//! domain, nor that the observation predates a reinstall. This module owns the
//! separate durable fact: one explicit epoch binding a trusted importer and
//! protocol to one exact resolved `(adapter_kind, adapter_binding_key)` domain.
//!
//! At most one epoch is ACTIVE per importer and domain. Registering a new epoch
//! retires the previous one at the new epoch's start, and retirement is final.
//! An attestation names the epoch it was taken against; the durable writer and
//! the acquisition transaction both require that epoch to be ACTIVE and to
//! match the attestation's importer, protocol and physical domain. Reinstalling
//! or moving the adapter therefore invalidates older attestations even inside
//! their one-hour validity window.

use crate::store::{map_sqlite, query_opt};
use agentype_core::{Error, UnixTime};
use rusqlite::{params, Transaction};

const SELECT_INSTALLATION: &str = "
    SELECT installation_id, importer_id, importer_protocol,
           adapter_kind, adapter_binding_key,
           installed_at, retired_at, recorded_at
    FROM enforcement_importer_installations";

/// One registered physical installation epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct ImporterInstallation {
    installation_id: String,
    importer_id: String,
    importer_protocol: String,
    adapter_kind: String,
    adapter_binding_key: String,
    installed_at: UnixTime,
    retired_at: Option<UnixTime>,
    recorded_at: UnixTime,
}

impl ImporterInstallation {
    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }

    pub fn importer_id(&self) -> &str {
        &self.importer_id
    }

    pub fn importer_protocol(&self) -> &str {
        &self.importer_protocol
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
    }

    pub fn installed_at(&self) -> UnixTime {
        self.installed_at
    }

    pub fn retired_at(&self) -> Option<UnixTime> {
        self.retired_at
    }

    pub fn recorded_at(&self) -> UnixTime {
        self.recorded_at
    }

    pub fn is_active(&self) -> bool {
        self.retired_at.is_none()
    }

    /// Whether this epoch is still the installed one at `now`. The interval is
    /// half-open: `installed_at <= now` and `now < retired_at`.
    pub fn is_active_at(&self, now: UnixTime) -> bool {
        now.is_finite()
            && now >= self.installed_at
            && self.retired_at.is_none_or(|retired_at| now < retired_at)
    }

    /// Whether this epoch binds exactly the attested importer, protocol and
    /// resolved physical domain.
    pub fn covers(
        &self,
        importer_id: &str,
        importer_protocol: &str,
        adapter_kind: &str,
        adapter_binding_key: &str,
    ) -> bool {
        self.importer_id == importer_id
            && self.importer_protocol == importer_protocol
            && self.adapter_kind == adapter_kind
            && self.adapter_binding_key == adapter_binding_key
    }

    /// The acquisition check: this is the ACTIVE epoch for the attested
    /// importer, protocol and physical domain, and the observation was issued
    /// inside the epoch.
    pub fn authorizes_observation(
        &self,
        importer_id: &str,
        importer_protocol: &str,
        adapter_kind: &str,
        adapter_binding_key: &str,
        issued_at: UnixTime,
        now: UnixTime,
    ) -> bool {
        self.is_active_at(now)
            && issued_at >= self.installed_at
            && self.covers(
                importer_id,
                importer_protocol,
                adapter_kind,
                adapter_binding_key,
            )
    }
}

/// Register one explicit installation epoch.
///
/// This is the trusted composition root's write path. Re-registering the exact
/// same row is idempotent. A new epoch for the same importer and physical domain
/// retires the previous ACTIVE epoch at `installed_at`; an epoch must not move
/// backwards. A different identity under the same `installation_id`, or an
/// epoch that starts before the previous one, fails closed.
#[allow(clippy::too_many_arguments)]
pub fn register_enforcement_importer_installation(
    tx: &Transaction<'_>,
    installation_id: &str,
    importer_id: &str,
    importer_protocol: &str,
    adapter_kind: &str,
    adapter_binding_key: &str,
    installed_at: UnixTime,
    recorded_at: UnixTime,
) -> Result<ImporterInstallation, Error> {
    for (value, field) in [
        (installation_id, "installation id"),
        (importer_id, "importer id"),
        (importer_protocol, "importer protocol"),
        (adapter_kind, "adapter kind"),
        (adapter_binding_key, "adapter binding key"),
    ] {
        if value.trim().is_empty() {
            return Err(Error::invalid_authority(format!(
                "a trusted-importer installation requires a nonblank {field}"
            )));
        }
    }
    if !installed_at.is_finite() || installed_at < 0.0 {
        return Err(Error::invariant(
            "a trusted-importer installation requires a finite epoch",
        ));
    }
    if !recorded_at.is_finite() || recorded_at < installed_at {
        return Err(Error::invariant(
            "a trusted-importer installation must be recorded at or after its epoch",
        ));
    }

    if let Some(existing) = enforcement_importer_installation(tx, installation_id)? {
        if existing.importer_id == importer_id
            && existing.importer_protocol == importer_protocol
            && existing.adapter_kind == adapter_kind
            && existing.adapter_binding_key == adapter_binding_key
            && existing.installed_at == installed_at
        {
            return Ok(existing);
        }
        return Err(Error::invariant(
            "one trusted-importer installation id cannot describe different epochs",
        ));
    }

    if let Some(previous) =
        active_installation_for_domain(tx, importer_id, adapter_kind, adapter_binding_key)?
    {
        if installed_at < previous.installed_at {
            return Err(Error::invariant(
                "a trusted-importer installation epoch cannot move backwards",
            ));
        }
        let retired = tx
            .execute(
                "UPDATE enforcement_importer_installations SET retired_at=?2
                 WHERE installation_id=?1 AND retired_at IS NULL",
                params![previous.installation_id, installed_at],
            )
            .map_err(map_sqlite)?;
        if retired != 1 {
            return Err(Error::invariant(
                "the previous trusted-importer installation epoch could not be retired",
            ));
        }
    }

    tx.execute(
        "INSERT INTO enforcement_importer_installations
         (installation_id, importer_id, importer_protocol,
          adapter_kind, adapter_binding_key, installed_at, retired_at, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        params![
            installation_id,
            importer_id,
            importer_protocol,
            adapter_kind,
            adapter_binding_key,
            installed_at,
            recorded_at,
        ],
    )
    .map_err(map_sqlite)?;
    enforcement_importer_installation(tx, installation_id)?
        .ok_or_else(|| Error::invariant("a registered trusted-importer installation disappeared"))
}

/// Retire an installation epoch. Returns whether the row changed; an absent or
/// already-retired row is `Ok(false)` when the requested retirement matches, and
/// an error when a different retirement is attempted. Retirement is final.
pub fn retire_enforcement_importer_installation(
    tx: &Transaction<'_>,
    installation_id: &str,
    retired_at: UnixTime,
) -> Result<bool, Error> {
    let Some(installation) = enforcement_importer_installation(tx, installation_id)? else {
        return Ok(false);
    };
    if !retired_at.is_finite() || retired_at < installation.installed_at {
        return Err(Error::invalid_transition(
            "a trusted-importer installation cannot be retired before its epoch",
        ));
    }
    match installation.retired_at {
        Some(existing) if existing == retired_at => Ok(false),
        Some(_) => Err(Error::invalid_transition(
            "a trusted-importer installation retirement is final",
        )),
        None => {
            let changed = tx
                .execute(
                    "UPDATE enforcement_importer_installations SET retired_at=?2
                     WHERE installation_id=?1 AND retired_at IS NULL",
                    params![installation_id, retired_at],
                )
                .map_err(map_sqlite)?;
            Ok(changed == 1)
        }
    }
}

/// The exact installation epoch, ACTIVE or retired.
pub fn enforcement_importer_installation(
    tx: &Transaction<'_>,
    installation_id: &str,
) -> Result<Option<ImporterInstallation>, Error> {
    query_opt(
        tx,
        &format!("{SELECT_INSTALLATION} WHERE installation_id=?1"),
        params![installation_id],
        raw_installation_row,
    )?
    .map(RawInstallation::into_installation)
    .transpose()
}

/// The exact installation epoch only while it is ACTIVE.
pub fn active_enforcement_importer_installation(
    tx: &Transaction<'_>,
    installation_id: &str,
) -> Result<Option<ImporterInstallation>, Error> {
    query_opt(
        tx,
        &format!("{SELECT_INSTALLATION} WHERE installation_id=?1 AND retired_at IS NULL"),
        params![installation_id],
        raw_installation_row,
    )?
    .map(RawInstallation::into_installation)
    .transpose()
}

/// The domain is `(importer, adapter_kind, adapter_binding_key)`, matching the
/// partial unique index. A changed importer protocol is a new epoch for the same
/// domain, so it retires the previous row exactly like a reinstall.
fn active_installation_for_domain(
    tx: &Transaction<'_>,
    importer_id: &str,
    adapter_kind: &str,
    adapter_binding_key: &str,
) -> Result<Option<ImporterInstallation>, Error> {
    query_opt(
        tx,
        &format!(
            "{SELECT_INSTALLATION} WHERE importer_id=?1 AND adapter_kind=?2 \
             AND adapter_binding_key=?3 AND retired_at IS NULL"
        ),
        params![importer_id, adapter_kind, adapter_binding_key],
        raw_installation_row,
    )?
    .map(RawInstallation::into_installation)
    .transpose()
}

struct RawInstallation {
    installation_id: String,
    importer_id: String,
    importer_protocol: String,
    adapter_kind: String,
    adapter_binding_key: String,
    installed_at: f64,
    retired_at: Option<f64>,
    recorded_at: f64,
}

fn raw_installation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawInstallation> {
    Ok(RawInstallation {
        installation_id: row.get(0)?,
        importer_id: row.get(1)?,
        importer_protocol: row.get(2)?,
        adapter_kind: row.get(3)?,
        adapter_binding_key: row.get(4)?,
        installed_at: row.get(5)?,
        retired_at: row.get(6)?,
        recorded_at: row.get(7)?,
    })
}

impl RawInstallation {
    fn into_installation(self) -> Result<ImporterInstallation, Error> {
        if !self.installed_at.is_finite()
            || self.installed_at < 0.0
            || !self.recorded_at.is_finite()
            || self.recorded_at < self.installed_at
        {
            return Err(Error::invariant(
                "a durable trusted-importer installation has an invalid epoch",
            ));
        }
        if let Some(retired_at) = self.retired_at {
            if !retired_at.is_finite() || retired_at < self.installed_at {
                return Err(Error::invariant(
                    "a durable trusted-importer installation has an invalid retirement",
                ));
            }
        }
        Ok(ImporterInstallation {
            installation_id: self.installation_id,
            importer_id: self.importer_id,
            importer_protocol: self.importer_protocol,
            adapter_kind: self.adapter_kind,
            adapter_binding_key: self.adapter_binding_key,
            installed_at: self.installed_at,
            retired_at: self.retired_at,
            recorded_at: self.recorded_at,
        })
    }
}
