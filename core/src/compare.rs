use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasurementMeaning {
    pub quantity: String,
    pub unit: String,
    pub method: String,
    pub population: String,
    pub denominator: String,
    pub aggregation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckOutcome {
    Passed,
    Failed,
    Missing,
}

#[derive(Clone, Debug)]
pub struct Trial {
    pub run: String,
    pub value: f64,
    pub conditions: BTreeMap<String, String>,
    pub complete_coverage: bool,
    pub required_output_checks: BTreeMap<String, CheckOutcome>,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub meaning: MeasurementMeaning,
    pub trials: Vec<Trial>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonStatus {
    Compatible,
    Conditional,
    Incompatible,
    UndefinedRatio,
}

#[derive(Clone, Debug)]
pub struct Comparison {
    pub status: ComparisonStatus,
    pub baseline_mean: f64,
    pub changed_mean: f64,
    pub difference: Option<f64>,
    pub ratio: Option<f64>,
    pub conditions: Vec<String>,
    pub declared_output_checks_preserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparisonError(pub &'static str);

impl fmt::Display for ComparisonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ComparisonError {}

fn mean(series: &Series) -> Result<f64, ComparisonError> {
    if series.trials.is_empty() || series.trials.len() > 10_000 {
        return Err(ComparisonError(
            "comparison requires between 1 and 10000 trials per series",
        ));
    }
    if series
        .trials
        .iter()
        .any(|t| !t.value.is_finite() || t.run.is_empty())
    {
        return Err(ComparisonError(
            "trial requires a run identity and a finite value",
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    if series.trials.iter().any(|t| !ids.insert(&t.run)) {
        return Err(ComparisonError("repetitions must refer to distinct runs"));
    }
    let n = series.trials.len() as f64;
    let value: f64 = series.trials.iter().map(|t| t.value / n).sum();
    value
        .is_finite()
        .then_some(value)
        .ok_or(ComparisonError("comparison arithmetic overflow"))
}

pub fn compare(baseline: &Series, changed: &Series) -> Result<Comparison, ComparisonError> {
    let baseline_mean = mean(baseline)?;
    let changed_mean = mean(changed)?;
    let mut result = Comparison {
        status: ComparisonStatus::Compatible,
        baseline_mean,
        changed_mean,
        difference: None,
        ratio: None,
        conditions: Vec::new(),
        declared_output_checks_preserved: false,
    };
    let a = &baseline.meaning;
    let b = &changed.meaning;
    let fields_a = [
        &a.quantity,
        &a.unit,
        &a.method,
        &a.population,
        &a.denominator,
        &a.aggregation,
    ];
    let fields_b = [
        &b.quantity,
        &b.unit,
        &b.method,
        &b.population,
        &b.denominator,
        &b.aggregation,
    ];
    if fields_a.iter().chain(fields_b.iter()).any(|s| s.is_empty()) {
        return Err(ComparisonError("measurement meaning must be explicit"));
    }
    if a != b {
        result.status = ComparisonStatus::Incompatible;
        result
            .conditions
            .push("measurement meaning, population or denominator differs".into());
        return Ok(result);
    }
    let trials: Vec<&Trial> = baseline.trials.iter().chain(&changed.trials).collect();
    if baseline.trials.len() < 2 || changed.trials.len() < 2 {
        result
            .conditions
            .push("fewer than two repetitions in a compared variant".into());
    }
    if trials.iter().any(|t| !t.complete_coverage) {
        result
            .conditions
            .push("incomplete observation coverage".into());
    }
    let expected_checks: Vec<&String> = baseline.trials[0].required_output_checks.keys().collect();
    let same_check_scopes = trials
        .iter()
        .all(|t| t.required_output_checks.keys().collect::<Vec<_>>() == expected_checks);
    result.declared_output_checks_preserved = !expected_checks.is_empty()
        && same_check_scopes
        && trials.iter().all(|t| {
            t.required_output_checks
                .values()
                .all(|v| *v == CheckOutcome::Passed)
        });
    if !result.declared_output_checks_preserved {
        result
            .conditions
            .push("required output checks are absent, failed or have different scopes".into());
    }
    if trials
        .iter()
        .any(|t| t.conditions != baseline.trials[0].conditions)
    {
        result
            .conditions
            .push("recorded inputs, builds or collection conditions differ".into());
    }
    let difference = changed_mean - baseline_mean;
    if !difference.is_finite() {
        return Err(ComparisonError("comparison arithmetic overflow"));
    }
    result.difference = Some(difference);
    if baseline_mean == 0.0 {
        result.status = ComparisonStatus::UndefinedRatio;
        result.conditions.push("zero baseline denominator".into());
        return Ok(result);
    }
    let ratio = changed_mean / baseline_mean;
    if !ratio.is_finite() {
        return Err(ComparisonError("comparison arithmetic overflow"));
    }
    result.ratio = Some(ratio);
    if !result.conditions.is_empty() {
        result.status = ComparisonStatus::Conditional;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(prefix: &str, values: &[f64]) -> Series {
        Series {
            meaning: MeasurementMeaning {
                quantity: "cpu".into(),
                unit: "ns".into(),
                method: "process counter delta".into(),
                population: "one process".into(),
                denominator: "same 100 assets".into(),
                aggregation: "sum user and system".into(),
            },
            trials: values
                .iter()
                .enumerate()
                .map(|(i, value)| Trial {
                    run: format!("{prefix}-{i}"),
                    value: *value,
                    conditions: BTreeMap::from([("input".into(), "asset-set-1".into())]),
                    complete_coverage: true,
                    required_output_checks: BTreeMap::from([(
                        "manifest and pixels".into(),
                        CheckOutcome::Passed,
                    )]),
                })
                .collect(),
        }
    }

    #[test]
    fn compared_repetitions_preserve_checks_and_descriptive_ratio() {
        let result = compare(&series("a", &[100., 120.]), &series("b", &[80., 90.])).unwrap();
        assert_eq!(result.status, ComparisonStatus::Compatible);
        assert_eq!(result.difference, Some(-25.));
        assert_eq!(result.ratio, Some(85. / 110.));
        assert!(result.declared_output_checks_preserved);
    }

    #[test]
    fn equal_units_cannot_combine_different_populations() {
        let a = series("a", &[100., 100.]);
        let mut b = series("b", &[10., 10.]);
        b.meaning.population = "one operation inferred from overlap".into();
        let result = compare(&a, &b).unwrap();
        assert_eq!(result.status, ComparisonStatus::Incompatible);
        assert_eq!(result.ratio, None);
        assert_eq!(result.difference, None);
    }

    #[test]
    fn failures_gaps_conditions_and_single_trial_prevent_unconditional_result() {
        let a = series("a", &[10., 10.]);
        let mut b = series("b", &[5.]);
        b.trials[0].complete_coverage = false;
        b.trials[0]
            .required_output_checks
            .insert("manifest and pixels".into(), CheckOutcome::Failed);
        b.trials[0]
            .conditions
            .insert("input".into(), "other assets".into());
        let result = compare(&a, &b).unwrap();
        assert_eq!(result.status, ComparisonStatus::Conditional);
        assert_eq!(result.conditions.len(), 4);
        assert!(!result.declared_output_checks_preserved);
    }

    #[test]
    fn zero_nonfinite_and_reused_repetition_are_explicit() {
        assert_eq!(
            compare(&series("a", &[0., 0.]), &series("b", &[1., 1.]))
                .unwrap()
                .status,
            ComparisonStatus::UndefinedRatio
        );
        assert!(compare(&series("a", &[f64::NAN]), &series("b", &[1.])).is_err());
        let mut repeated = series("a", &[10., 10.]);
        repeated.trials[1].run = repeated.trials[0].run.clone();
        assert!(compare(&repeated, &series("b", &[1., 1.])).is_err());
    }
}
