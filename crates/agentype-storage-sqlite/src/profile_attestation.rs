//! M6-B.5 B5.2 durable trusted-importer attestations (schema v11).
//!
//! A profile digest proves only that an imported profile document is internally
//! consistent: the same caller that supplies the support matrix, covered
//! subjects and physical binding key also computes the digest. These rows are
//! the separate external fact. The provisioning producer writes one immutable
//! row per attestation, keyed by its own digest and indexed by the exact profile
//! digest it covers, and binds it to the importer identity, protocol, exact
//! physical adapter domain and validity window.
//!
//! Acquisition reads the freshest row valid at its clock. A profile with no such
//! row is rejected before any qualification or Attempt/Lease, so a caller cannot
//! widen support, forge subject coverage, replay stale physical-domain evidence,
//! or move the physical key under a stable alias and still be accepted.

use crate::store::{map_sqlite, query_opt};
use agentype_agent_contract::EnforcementProfileAttestation;
use agentype_core::{Error, UnixTime};
use rusqlite::{params, Transaction};

const SELECT_ATTESTATIONS: &str = "
    SELECT attestation_digest, profile_digest, importer_id, importer_protocol,
           adapter_kind, adapter_binding_key, issued_at, valid_until
    FROM enforcement_profile_attestations";

/// Record one immutable trusted-importer attestation.
///
/// `recorded_at` is the durable write time; it does not affect freshness, so an
/// already-expired historical attestation may still be recorded. Re-recording
/// the same attestation is an idempotent no-op. Because the attestation digest
/// commits to every field, a second row under the same digest cannot describe
/// different facts; a raw writer that achieves one is rejected on read.
pub fn record_enforcement_profile_attestation(
    tx: &Transaction<'_>,
    attestation: &EnforcementProfileAttestation,
    recorded_at: UnixTime,
) -> Result<(), Error> {
    if !recorded_at.is_finite() || recorded_at < 0.0 {
        return Err(Error::invariant(
            "a trusted-importer attestation requires a finite write time",
        ));
    }
    let existing = query_opt(
        tx,
        &format!("{SELECT_ATTESTATIONS} WHERE attestation_digest=?1"),
        params![attestation.attestation_digest()],
        raw_attestation_row,
    )?
    .map(RawAttestation::into_attestation)
    .transpose()?;
    if let Some(existing) = existing {
        if &existing != attestation {
            return Err(Error::invariant(
                "a trusted-importer attestation digest describes different durable parts",
            ));
        }
        return Ok(());
    }
    tx.execute(
        "INSERT INTO enforcement_profile_attestations
         (attestation_digest, profile_digest, importer_id, importer_protocol,
          adapter_kind, adapter_binding_key, issued_at, valid_until, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            attestation.attestation_digest(),
            attestation.profile_digest(),
            attestation.importer_id(),
            attestation.importer_protocol(),
            attestation.adapter_kind(),
            attestation.adapter_binding_key(),
            attestation.issued_at(),
            attestation.valid_until(),
            recorded_at,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// The most recently issued attestation for an exact profile digest, without a
/// freshness check.
///
/// This is the validated operator/diagnostic read. The row is rebuilt through
/// [`EnforcementProfileAttestation::from_parts`], so a tampered digest or a
/// domain that disagrees with its own parts fails closed instead of being
/// returned as a trusted fact.
pub fn enforcement_profile_attestation(
    tx: &Transaction<'_>,
    profile_digest: &str,
) -> Result<Option<EnforcementProfileAttestation>, Error> {
    query_opt(
        tx,
        &format!(
            "{SELECT_ATTESTATIONS} WHERE profile_digest=?1 \
             ORDER BY issued_at DESC, attestation_digest DESC LIMIT 1"
        ),
        params![profile_digest],
        raw_attestation_row,
    )?
    .map(RawAttestation::into_attestation)
    .transpose()
}

/// The freshest attestation valid at `now` for an exact profile digest.
///
/// The SQL predicate is only an index filter; the returned row is rebuilt and
/// re-checked with [`EnforcementProfileAttestation::is_fresh`] before it is
/// used. An absent row, an expired row, a not-yet-issued row and a non-finite
/// clock all yield `None`; the acquisition caller decides how to reject.
pub fn current_enforcement_profile_attestation(
    tx: &Transaction<'_>,
    profile_digest: &str,
    now: UnixTime,
) -> Result<Option<EnforcementProfileAttestation>, Error> {
    if !now.is_finite() {
        return Ok(None);
    }
    let attestation = query_opt(
        tx,
        &format!(
            "{SELECT_ATTESTATIONS} WHERE profile_digest=?1 AND issued_at<=?2 \
             AND valid_until>?2 ORDER BY issued_at DESC, attestation_digest DESC LIMIT 1"
        ),
        params![profile_digest, now],
        raw_attestation_row,
    )?
    .map(RawAttestation::into_attestation)
    .transpose()?;
    Ok(attestation.filter(|attestation| attestation.is_fresh(now)))
}

struct RawAttestation {
    attestation_digest: String,
    profile_digest: String,
    importer_id: String,
    importer_protocol: String,
    adapter_kind: String,
    adapter_binding_key: String,
    issued_at: f64,
    valid_until: f64,
}

fn raw_attestation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAttestation> {
    Ok(RawAttestation {
        attestation_digest: row.get(0)?,
        profile_digest: row.get(1)?,
        importer_id: row.get(2)?,
        importer_protocol: row.get(3)?,
        adapter_kind: row.get(4)?,
        adapter_binding_key: row.get(5)?,
        issued_at: row.get(6)?,
        valid_until: row.get(7)?,
    })
}

impl RawAttestation {
    fn into_attestation(self) -> Result<EnforcementProfileAttestation, Error> {
        EnforcementProfileAttestation::from_parts(
            self.profile_digest,
            self.importer_id,
            self.importer_protocol,
            self.adapter_kind,
            self.adapter_binding_key,
            self.issued_at,
            self.valid_until,
            &self.attestation_digest,
        )
        .map_err(|error| Error::invariant(error.to_string()))
    }
}
