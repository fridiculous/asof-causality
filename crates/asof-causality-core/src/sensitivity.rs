use crate::{
    blake3_hex, Event, EventKey, EventRole, PredictionRecord, ReplayEngine, ReplayOptions,
    ReplayOrder, ReplayOutput, Signal,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyCategory {
    Baseline,
    SyntheticStress,
    RealisticFailure,
}

impl PolicyCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::SyntheticStress => "synthetic_stress",
            Self::RealisticFailure => "realistic_failure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyKind {
    Identity,
    ReceivedTimeShift {
        roles_affected: Vec<EventRole>,
        shift: i64,
    },
    ReceivedTimeLagFraction {
        roles_affected: Vec<EventRole>,
        pct_bps: u16,
    },
    ReceivedTimeLagBucketLookahead {
        roles_affected: Vec<EventRole>,
        min_lag: u64,
        max_lag_exclusive: Option<u64>,
        pct_bps: u16,
    },
    ReceivedTimeLagCumulativeLookahead {
        roles_affected: Vec<EventRole>,
        max_lag_inclusive: Option<u64>,
        threshold_pct_bps: u16,
        pct_bps: u16,
    },
    ReplayOrderOverride {
        order: ReplayOrder,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyPoint {
    pub name: String,
    pub category: PolicyCategory,
    pub kind: PolicyKind,
}

impl PolicyPoint {
    pub fn strict_received_time() -> Self {
        Self {
            name: "strict_received_time".to_string(),
            category: PolicyCategory::Baseline,
            kind: PolicyKind::Identity,
        }
    }

    pub fn shift_features(name: impl Into<String>, shift: i64) -> Self {
        Self {
            name: name.into(),
            category: PolicyCategory::SyntheticStress,
            kind: PolicyKind::ReceivedTimeShift {
                roles_affected: vec![EventRole::Feature, EventRole::FeatureCorrection],
                shift,
            },
        }
    }

    pub fn leak_feature_lag_fraction(name: impl Into<String>, pct_bps: u16) -> Self {
        Self {
            name: name.into(),
            category: PolicyCategory::SyntheticStress,
            kind: PolicyKind::ReceivedTimeLagFraction {
                roles_affected: vec![EventRole::Feature, EventRole::FeatureCorrection],
                pct_bps: pct_bps.min(10_000),
            },
        }
    }

    pub fn leak_feature_lag_bucket(
        name: impl Into<String>,
        min_lag: u64,
        max_lag_exclusive: Option<u64>,
        pct_bps: u16,
    ) -> Self {
        Self {
            name: name.into(),
            category: PolicyCategory::SyntheticStress,
            kind: PolicyKind::ReceivedTimeLagBucketLookahead {
                roles_affected: vec![EventRole::Feature, EventRole::FeatureCorrection],
                min_lag,
                max_lag_exclusive,
                pct_bps: pct_bps.min(10_000),
            },
        }
    }

    pub fn leak_feature_lag_cumulative(
        name: impl Into<String>,
        max_lag_inclusive: Option<u64>,
        threshold_pct_bps: u16,
        pct_bps: u16,
    ) -> Self {
        Self {
            name: name.into(),
            category: PolicyCategory::SyntheticStress,
            kind: PolicyKind::ReceivedTimeLagCumulativeLookahead {
                roles_affected: vec![EventRole::Feature, EventRole::FeatureCorrection],
                max_lag_inclusive,
                threshold_pct_bps: threshold_pct_bps.min(10_000),
                pct_bps: pct_bps.min(10_000),
            },
        }
    }

    pub fn observed_time_leaky() -> Self {
        Self {
            name: "observed_time_leaky".to_string(),
            category: PolicyCategory::RealisticFailure,
            kind: PolicyKind::ReplayOrderOverride {
                order: ReplayOrder::ObservedTimeLeaky,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRun {
    pub policy: PolicyPoint,
    pub events_transformed: usize,
    pub transformed_fixture_hash: String,
    pub output: ReplayOutput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SensitivitySummary {
    pub predictions: usize,
    pub predictions_with_signal_change: usize,
    pub predictions_with_recipe_change: usize,
    pub predictions_with_new_inputs: usize,
    pub new_input_uses: usize,
    pub unique_new_inputs: usize,
    pub flip_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitivityDetail {
    pub prediction_event_key: EventKey,
    pub baseline: PredictionRecord,
    pub comparison: PredictionRecord,
    pub new_inputs_admitted: Vec<EventKey>,
    pub signal_value_changed: bool,
    pub feature_recipe_hash_changed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SensitivityPolicyResult {
    pub run: PolicyRun,
    pub summary: SensitivitySummary,
    pub details: Vec<SensitivityDetail>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SensitivitySweep {
    pub baseline: PolicyRun,
    pub results: Vec<SensitivityPolicyResult>,
}

pub fn run_sensitivity_sweep<S>(
    events: &[Event],
    comparison_policies: &[PolicyPoint],
    signal: S,
) -> Result<SensitivitySweep, SensitivityError>
where
    S: Signal + Clone,
{
    if comparison_policies.is_empty() {
        return Err(SensitivityError::NoComparisonPolicies);
    }
    ensure_unique_policy_names(comparison_policies)?;

    let baseline_policy = PolicyPoint::strict_received_time();
    let baseline = run_policy(events, &baseline_policy, &signal)?;
    let mut results = Vec::with_capacity(comparison_policies.len());

    for policy in comparison_policies {
        let run = run_policy(events, policy, &signal)?;
        let (summary, details) = diff_against_baseline(&baseline, &run)?;
        results.push(SensitivityPolicyResult {
            run,
            summary,
            details,
        });
    }

    Ok(SensitivitySweep { baseline, results })
}

pub fn transform_events_for_policy(
    events: &[Event],
    policy: &PolicyPoint,
) -> Result<(Vec<Event>, usize, ReplayOrder), SensitivityError> {
    match &policy.kind {
        PolicyKind::Identity => Ok((events.to_vec(), 0, ReplayOrder::ReceivedTime)),
        PolicyKind::ReplayOrderOverride { order } => Ok((events.to_vec(), 0, *order)),
        PolicyKind::ReceivedTimeShift {
            roles_affected,
            shift,
        } => {
            let mut transformed = Vec::with_capacity(events.len());
            let mut events_transformed = 0;

            for event in events {
                let mut event = event.clone();
                if roles_affected.contains(&event.role) {
                    event.received_time = shift_time(event.received_time, *shift, &policy.name)?;
                    events_transformed += 1;
                }
                transformed.push(event);
            }

            Ok((transformed, events_transformed, ReplayOrder::ReceivedTime))
        }
        PolicyKind::ReceivedTimeLagFraction {
            roles_affected,
            pct_bps,
        } => {
            let mut transformed = Vec::with_capacity(events.len());
            let mut events_transformed = 0;

            for event in events {
                let mut event = event.clone();
                if roles_affected.contains(&event.role) {
                    let shifted = leak_received_time_toward_observed_time(
                        event.observed_time,
                        event.received_time,
                        *pct_bps,
                    );
                    if shifted != event.received_time {
                        event.received_time = shifted;
                        events_transformed += 1;
                    }
                }
                transformed.push(event);
            }

            Ok((transformed, events_transformed, ReplayOrder::ReceivedTime))
        }
        PolicyKind::ReceivedTimeLagBucketLookahead {
            roles_affected,
            min_lag,
            max_lag_exclusive,
            pct_bps,
        } => {
            let mut transformed = Vec::with_capacity(events.len());
            let mut events_transformed = 0;

            for event in events {
                let mut event = event.clone();
                if roles_affected.contains(&event.role)
                    && event.received_time > event.observed_time
                    && lag_is_in_bucket(
                        event.received_time - event.observed_time,
                        *min_lag,
                        *max_lag_exclusive,
                    )
                {
                    let shifted = leak_received_time_toward_observed_time(
                        event.observed_time,
                        event.received_time,
                        *pct_bps,
                    );
                    if shifted != event.received_time {
                        event.received_time = shifted;
                        events_transformed += 1;
                    }
                }
                transformed.push(event);
            }

            Ok((transformed, events_transformed, ReplayOrder::ReceivedTime))
        }
        PolicyKind::ReceivedTimeLagCumulativeLookahead {
            roles_affected,
            max_lag_inclusive,
            pct_bps,
            ..
        } => {
            let mut transformed = Vec::with_capacity(events.len());
            let mut events_transformed = 0;

            for event in events {
                let mut event = event.clone();
                if roles_affected.contains(&event.role)
                    && event.received_time > event.observed_time
                    && lag_is_at_or_below_threshold(
                        event.received_time - event.observed_time,
                        *max_lag_inclusive,
                    )
                {
                    let shifted = leak_received_time_toward_observed_time(
                        event.observed_time,
                        event.received_time,
                        *pct_bps,
                    );
                    if shifted != event.received_time {
                        event.received_time = shifted;
                        events_transformed += 1;
                    }
                }
                transformed.push(event);
            }

            Ok((transformed, events_transformed, ReplayOrder::ReceivedTime))
        }
    }
}

pub fn canonical_event_stream_hash(events: &[Event]) -> String {
    let mut text = String::new();
    for event in events {
        text.push_str(&event.to_pipe_record());
        text.push('\n');
    }
    blake3_hex(text.as_bytes())
}

fn run_policy<S>(
    events: &[Event],
    policy: &PolicyPoint,
    signal: &S,
) -> Result<PolicyRun, SensitivityError>
where
    S: Signal + Clone,
{
    let (transformed, events_transformed, order) = transform_events_for_policy(events, policy)?;
    let transformed_fixture_hash = canonical_event_stream_hash(&transformed);
    let output = ReplayEngine::with_signal(signal.clone())
        .replay_with_order(&transformed, ReplayOptions::default(), order)
        .map_err(|error| SensitivityError::Replay {
            policy_name: policy.name.clone(),
            message: error.to_string(),
        })?;

    Ok(PolicyRun {
        policy: policy.clone(),
        events_transformed,
        transformed_fixture_hash,
        output,
    })
}

fn ensure_unique_policy_names(policies: &[PolicyPoint]) -> Result<(), SensitivityError> {
    let mut names = BTreeSet::from([PolicyPoint::strict_received_time().name]);
    for policy in policies {
        if !names.insert(policy.name.clone()) {
            return Err(SensitivityError::DuplicatePolicyName(policy.name.clone()));
        }
    }
    Ok(())
}

fn shift_time(received_time: u64, shift: i64, policy_name: &str) -> Result<u64, SensitivityError> {
    let shifted = i128::from(received_time) + i128::from(shift);
    if shifted < 0 || shifted > i128::from(u64::MAX) {
        return Err(SensitivityError::TimestampShiftOutOfRange {
            policy_name: policy_name.to_string(),
            received_time,
            shift,
        });
    }
    Ok(shifted as u64)
}

fn leak_received_time_toward_observed_time(
    observed_time: u64,
    received_time: u64,
    pct_bps: u16,
) -> u64 {
    if received_time <= observed_time || pct_bps == 0 {
        return received_time;
    }

    let bounded_pct_bps = u128::from(pct_bps.min(10_000));
    let lag = u128::from(received_time - observed_time);
    let leaked_lag = ((lag * bounded_pct_bps) + 5_000) / 10_000;
    received_time - leaked_lag as u64
}

fn lag_is_in_bucket(lag: u64, min_lag: u64, max_lag_exclusive: Option<u64>) -> bool {
    lag >= min_lag
        && match max_lag_exclusive {
            Some(max_lag) => lag < max_lag,
            None => true,
        }
}

fn lag_is_at_or_below_threshold(lag: u64, max_lag_inclusive: Option<u64>) -> bool {
    max_lag_inclusive
        .map(|threshold| lag <= threshold)
        .unwrap_or(true)
}

fn diff_against_baseline(
    baseline: &PolicyRun,
    comparison: &PolicyRun,
) -> Result<(SensitivitySummary, Vec<SensitivityDetail>), SensitivityError> {
    let baseline_records = prediction_map(baseline.output.predictions.records());
    let comparison_records = prediction_map(comparison.output.predictions.records());
    ensure_matching_prediction_keys(
        &comparison.policy.name,
        &baseline_records,
        &comparison_records,
    )?;

    let mut predictions_with_signal_change = 0;
    let mut predictions_with_recipe_change = 0;
    let mut predictions_with_new_inputs = 0;
    let mut new_input_uses = 0;
    let mut unique_new_inputs = BTreeSet::new();
    let mut details = Vec::new();

    for baseline_record in baseline.output.predictions.records() {
        let prediction_event_key = baseline_record.prediction_event_key;
        let comparison_record = comparison_records
            .get(&prediction_event_key)
            .expect("prediction key equality is checked before diffing");
        let signal_value_changed = baseline_record.signal_value != comparison_record.signal_value;
        let feature_recipe_hash_changed =
            baseline_record.feature_recipe_hash != comparison_record.feature_recipe_hash;
        let new_inputs_admitted = new_inputs(
            baseline_record.input_event_ids_used,
            comparison_record.input_event_ids_used,
        );

        if signal_value_changed {
            predictions_with_signal_change += 1;
        }
        if feature_recipe_hash_changed {
            predictions_with_recipe_change += 1;
        }
        if !new_inputs_admitted.is_empty() {
            predictions_with_new_inputs += 1;
            new_input_uses += new_inputs_admitted.len();
            unique_new_inputs.extend(new_inputs_admitted.iter().copied());
        }

        if signal_value_changed || feature_recipe_hash_changed || !new_inputs_admitted.is_empty() {
            details.push(SensitivityDetail {
                prediction_event_key,
                baseline: baseline_record.clone(),
                comparison: (*comparison_record).clone(),
                new_inputs_admitted,
                signal_value_changed,
                feature_recipe_hash_changed,
            });
        }
    }

    let predictions = baseline.output.predictions.records().len();
    let flip_rate = if predictions == 0 {
        0.0
    } else {
        predictions_with_signal_change as f64 / predictions as f64
    };

    Ok((
        SensitivitySummary {
            predictions,
            predictions_with_signal_change,
            predictions_with_recipe_change,
            predictions_with_new_inputs,
            new_input_uses,
            unique_new_inputs: unique_new_inputs.len(),
            flip_rate,
        },
        details,
    ))
}

fn prediction_map(records: &[PredictionRecord]) -> BTreeMap<EventKey, &PredictionRecord> {
    records
        .iter()
        .map(|record| (record.prediction_event_key, record))
        .collect()
}

fn ensure_matching_prediction_keys(
    policy_name: &str,
    baseline: &BTreeMap<EventKey, &PredictionRecord>,
    comparison: &BTreeMap<EventKey, &PredictionRecord>,
) -> Result<(), SensitivityError> {
    let missing = baseline
        .keys()
        .filter(|key| !comparison.contains_key(key))
        .count();
    let extra = comparison
        .keys()
        .filter(|key| !baseline.contains_key(key))
        .count();

    if missing == 0 && extra == 0 {
        Ok(())
    } else {
        Err(SensitivityError::PredictionKeyMismatch {
            policy_name: policy_name.to_string(),
            missing,
            extra,
        })
    }
}

fn new_inputs(baseline: crate::InputSet, comparison: crate::InputSet) -> Vec<EventKey> {
    comparison
        .iter()
        .filter(|event_key| !baseline.contains_key(*event_key))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensitivityError {
    NoComparisonPolicies,
    DuplicatePolicyName(String),
    TimestampShiftOutOfRange {
        policy_name: String,
        received_time: u64,
        shift: i64,
    },
    PredictionKeyMismatch {
        policy_name: String,
        missing: usize,
        extra: usize,
    },
    Replay {
        policy_name: String,
        message: String,
    },
}

impl fmt::Display for SensitivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoComparisonPolicies => {
                write!(f, "sensitivity requires at least one comparison policy")
            }
            Self::DuplicatePolicyName(name) => write!(f, "duplicate sensitivity policy: {name}"),
            Self::TimestampShiftOutOfRange {
                policy_name,
                received_time,
                shift,
            } => write!(
                f,
                "policy {policy_name} shifts received_time {received_time} by {shift} outside u64 range"
            ),
            Self::PredictionKeyMismatch {
                policy_name,
                missing,
                extra,
            } => write!(
                f,
                "policy {policy_name} produced prediction key mismatch: missing={missing} extra={extra}"
            ),
            Self::Replay {
                policy_name,
                message,
            } => write!(f, "policy {policy_name} replay failed: {message}"),
        }
    }
}

impl Error for SensitivityError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_pipe_events, WindowedFeatureSentimentSignal, WindowedZScoreSignal};

    #[test]
    fn shift_transforms_only_selected_roles_and_preserves_identity() {
        let events = parse_pipe_events(
            "\
f1|100|100|1|feature|XYZ|sentiment=positive
p1|110|110|2|prediction|XYZ|
c1|120|120|3|feature_correction|XYZ|sentiment=negative
o1|130|130|4|outcome|XYZ|return_bps=1
",
        )
        .unwrap();
        let policy = PolicyPoint::shift_features("shift_features_minus_10", -10);
        let (transformed, count, order) = transform_events_for_policy(&events, &policy).unwrap();

        assert_eq!(count, 2);
        assert_eq!(order, ReplayOrder::ReceivedTime);
        assert_eq!(transformed[0].event_id, events[0].event_id);
        assert_eq!(transformed[0].observed_time, events[0].observed_time);
        assert_eq!(transformed[0].sequence, events[0].sequence);
        assert_eq!(transformed[0].symbol, events[0].symbol);
        assert_eq!(transformed[0].payload, events[0].payload);
        assert_eq!(transformed[0].received_time, 90);
        assert_eq!(transformed[1].received_time, 110);
        assert_eq!(transformed[2].received_time, 110);
        assert_eq!(transformed[3].received_time, 130);
    }

    #[test]
    fn shift_rejects_underflow() {
        let events = parse_pipe_events("f1|1|1|1|feature|XYZ|sentiment=positive\n").unwrap();
        let policy = PolicyPoint::shift_features("shift_features_minus_10", -10);
        let error = transform_events_for_policy(&events, &policy).unwrap_err();

        assert!(matches!(
            error,
            SensitivityError::TimestampShiftOutOfRange { .. }
        ));
    }

    #[test]
    fn lag_fraction_moves_each_event_toward_its_own_observed_time() {
        let events = parse_pipe_events(
            "\
f1|100|200|1|feature|XYZ|sentiment=positive
f2|100|140|2|feature|XYZ|sentiment=negative
p1|150|150|3|prediction|XYZ|
",
        )
        .unwrap();
        let policy = PolicyPoint::leak_feature_lag_fraction("leakage_50pct", 5_000);
        let (transformed, events_transformed, order) =
            transform_events_for_policy(&events, &policy).unwrap();

        assert_eq!(order, ReplayOrder::ReceivedTime);
        assert_eq!(events_transformed, 2);
        assert_eq!(transformed[0].received_time, 150);
        assert_eq!(transformed[1].received_time, 120);
        assert_eq!(transformed[2].received_time, 150);
    }

    #[test]
    fn full_lag_fraction_never_moves_before_observed_time() {
        let events = parse_pipe_events(
            "\
f1|100|200|1|feature|XYZ|sentiment=positive
f2|140|140|2|feature|XYZ|sentiment=negative
",
        )
        .unwrap();
        let policy = PolicyPoint::leak_feature_lag_fraction("leakage_100pct", 10_000);
        let (transformed, events_transformed, _) =
            transform_events_for_policy(&events, &policy).unwrap();

        assert_eq!(events_transformed, 1);
        assert_eq!(transformed[0].received_time, 100);
        assert_eq!(transformed[1].received_time, 140);
    }

    #[test]
    fn lag_bucket_lookahead_only_moves_events_inside_bucket() {
        let events = parse_pipe_events(
            "\
f1|100|120|1|feature|XYZ|sentiment=positive
f2|100|175|2|feature|XYZ|sentiment=negative
f3|100|240|3|feature|XYZ|sentiment=positive
p1|250|250|4|prediction|XYZ|
",
        )
        .unwrap();
        let policy = PolicyPoint::leak_feature_lag_bucket(
            "late_arrivals_lag_50_to_99",
            50,
            Some(100),
            10_000,
        );
        let (transformed, events_transformed, order) =
            transform_events_for_policy(&events, &policy).unwrap();

        assert_eq!(order, ReplayOrder::ReceivedTime);
        assert_eq!(events_transformed, 1);
        assert_eq!(transformed[0].received_time, 120);
        assert_eq!(transformed[1].received_time, 100);
        assert_eq!(transformed[2].received_time, 240);
        assert_eq!(transformed[3].received_time, 250);
    }

    #[test]
    fn cumulative_lag_lookahead_moves_all_events_up_to_threshold() {
        let events = parse_pipe_events(
            "\
f1|100|120|1|feature|XYZ|sentiment=positive
f2|100|175|2|feature|XYZ|sentiment=negative
f3|100|240|3|feature|XYZ|sentiment=positive
p1|250|250|4|prediction|XYZ|
",
        )
        .unwrap();
        let policy = PolicyPoint::leak_feature_lag_cumulative(
            "late_arrivals_cumulative_50pct",
            Some(75),
            5_000,
            10_000,
        );
        let (transformed, events_transformed, order) =
            transform_events_for_policy(&events, &policy).unwrap();

        assert_eq!(order, ReplayOrder::ReceivedTime);
        assert_eq!(events_transformed, 2);
        assert_eq!(transformed[0].received_time, 100);
        assert_eq!(transformed[1].received_time, 100);
        assert_eq!(transformed[2].received_time, 240);
        assert_eq!(transformed[3].received_time, 250);
    }

    #[test]
    fn duplicate_policy_names_fail() {
        let events = parse_pipe_events("f1|1|1|1|feature|XYZ|sentiment=positive\n").unwrap();
        let policies = [
            PolicyPoint::shift_features("same", -1),
            PolicyPoint::shift_features("same", -2),
        ];
        let error =
            run_sensitivity_sweep(&events, &policies, WindowedFeatureSentimentSignal::new(5))
                .unwrap_err();

        assert_eq!(
            error,
            SensitivityError::DuplicatePolicyName("same".to_string())
        );
    }

    #[test]
    fn diff_counts_signal_recipe_and_new_inputs() {
        let events = parse_pipe_events(
            "\
f1|100|100|1|feature|XYZ|score=10
f2|110|110|2|feature|XYZ|score=10
f3|120|120|3|feature|XYZ|score=10
p1|130|130|4|prediction|XYZ|
f4|125|135|5|feature|XYZ|score=30
p2|140|140|6|prediction|XYZ|
",
        )
        .unwrap();
        let policies = [PolicyPoint::shift_features("shift_features_minus_10", -10)];
        let sweep = run_sensitivity_sweep(&events, &policies, WindowedZScoreSignal::new()).unwrap();
        let result = &sweep.results[0];

        assert_eq!(result.run.events_transformed, 4);
        assert_eq!(result.summary.predictions, 2);
        assert_eq!(result.summary.predictions_with_signal_change, 1);
        assert_eq!(result.summary.predictions_with_recipe_change, 1);
        assert_eq!(result.summary.predictions_with_new_inputs, 1);
        assert_eq!(result.summary.new_input_uses, 1);
        assert_eq!(result.summary.unique_new_inputs, 1);
        assert_eq!(result.details.len(), 1);
    }
}
