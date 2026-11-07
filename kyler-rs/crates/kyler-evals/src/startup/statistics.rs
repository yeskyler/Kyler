use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::Sample;
use crate::report::distribution;

pub fn summarize(samples: &[Sample]) -> Value {
    let mut summary = group(
        &samples
            .iter()
            .filter(|s| s.variant == "candidate")
            .collect::<Vec<_>>(),
    );
    let names: BTreeSet<_> = samples.iter().map(|s| s.variant.as_str()).collect();
    if names.len() > 1 {
        let variants: BTreeMap<_, _> = names
            .into_iter()
            .map(|name| {
                (
                    name,
                    group(
                        &samples
                            .iter()
                            .filter(|s| s.variant == name)
                            .collect::<Vec<_>>(),
                    ),
                )
            })
            .collect();
        summary["variants"] = json!(variants);
        let baseline = &variants["baseline"];
        let candidate = &variants["candidate"];
        let comparison:BTreeMap<_,_>=["/service_cold_ms/p50","/service_cold_ms/p95","/process_to_main_ms/p50","/peak_rss_bytes/p50"].into_iter().map(|path|{
            let values=baseline.pointer(path).and_then(Value::as_f64).zip(candidate.pointer(path).and_then(Value::as_f64));
            (path,json!({"candidate_minus_baseline":values.map(|(a,b)|b-a),"change_percent":values.and_then(|(a,b)|(a!=0.0).then_some((b-a)/a*100.0))}))
        }).collect();
        summary["alternating_comparison"] = json!(comparison);
    }
    summary
}
fn group(samples: &[&Sample]) -> Value {
    let measured: Vec<_> = samples.iter().copied().filter(|s| !s.warmup).collect();
    let completed: Vec<_> = measured
        .iter()
        .copied()
        .filter(|s| s.status == "completed")
        .collect();
    let successful: Vec<_> = completed
        .iter()
        .copied()
        .filter(|s| s.measurement.as_ref().is_none_or(|m| m.degraded.is_empty()))
        .collect();
    let point = |name: &str| {
        distribution(
            successful
                .iter()
                .filter_map(|s| s.timeline_ms.get(name).copied()),
        )
    };
    let interval = |start: &str, end: &str| {
        distribution(successful.iter().filter_map(|s| {
            let elapsed = *s.timeline_ms.get(end)? - *s.timeline_ms.get(start)?;
            (elapsed >= 0.0).then_some(elapsed)
        }))
    };
    let phase_names: BTreeSet<_> = successful
        .iter()
        .filter_map(|s| s.measurement.as_ref())
        .flat_map(|m| m.phases_ms.keys().cloned())
        .collect();
    let phases: BTreeMap<_, _> = phase_names
        .into_iter()
        .map(|name| {
            let values = distribution(
                successful
                    .iter()
                    .filter_map(|s| s.measurement.as_ref()?.phases_ms.get(&name).copied()),
            );
            (name, values)
        })
        .collect();
    json!({
        "kind":"startup","samples":measured.len(),"successful":completed.len(),"latency_samples":successful.len(),"failed":measured.iter().filter(|s|s.status=="failed").count(),"unsuccessful":measured.len()-completed.len(),
        "timed_out":measured.iter().filter(|s|s.status=="timed_out").count(),"cancelled":measured.iter().filter(|s|s.status=="cancelled").count(),
        "success_rate":(!measured.is_empty()).then(||completed.len() as f64/measured.len() as f64),
        "degraded":completed.iter().filter(|s|s.measurement.as_ref().is_some_and(|m|!m.degraded.is_empty())).count(),
        "capabilities_evaluated":completed.iter().filter(|s|s.measurement.as_ref().is_some_and(|m|m.capabilities_evaluated)).count(),
        "degraded_turn_to_first_text_ms":distribution(completed.iter().filter(|s|s.measurement.as_ref().is_some_and(|m|!m.degraded.is_empty())).filter_map(|s|s.measurement.as_ref()?.first_text_ms)),
        "warmup_samples":samples.iter().filter(|s|s.warmup).count(),"warmup_failed":samples.iter().filter(|s|s.warmup && s.status!="completed").count(),
        "process_to_main_ms":point("main_entered"),"service_cold_ms":point("service_ready"),"agent_ready_ms":point("agent_ready"),"capabilities_ready_ms":point("capabilities_ready"),
        "runtime_ready_ms":point("runtime_ready"),"process_to_ready_ms":distribution(successful.iter().filter_map(|s|s.process_to_ready_ms)),
        "turn_to_capabilities_ms":interval("turn_submitted","capabilities_ready"),
        "turn_to_request_ms":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.timings.first_request_ms)),
        "request_to_first_event_ms":distribution(successful.iter().filter_map(|s|{let t=&s.measurement.as_ref()?.timings;Some(t.first_model_event_ms?-t.first_request_ms?)})),
        "request_to_first_text_ms":interval("model_request_started","first_text"),
        "turn_to_first_text_ms":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.first_text_ms)),
        "startup_to_first_text_ms":point("first_text"),"turn_total_ms":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.turn_ms)),
        "restore_ms":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.restore_ms)),
        "unmeasured_prime_turn_ms":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.warmup_turn_ms)),
        "peak_rss_bytes":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.peak_rss_bytes).map(|n|n as f64)),
        "cpu_seconds":distribution(successful.iter().filter_map(|s|s.measurement.as_ref()?.cpu_seconds)),"phases_ms":phases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(variant: &str, warmup: bool, status: &str, ready: f64) -> Sample {
        Sample {
            index: 0,
            variant: variant.into(),
            warmup,
            status: status.into(),
            process_to_ready_ms: Some(ready),
            process_total_ms: ready,
            timeline_ms: BTreeMap::from([("service_ready".into(), ready)]),
            build_profile: Some("release".into()),
            measurement: None,
            error: None,
        }
    }
    #[test]
    fn failures_and_warmups_never_improve_latency_percentiles() {
        let summary = summarize(&[
            sample("candidate", true, "completed", 1.0),
            sample("candidate", false, "failed", 2.0),
            sample("candidate", false, "completed", 100.0),
        ]);
        assert_eq!(summary["samples"], 2);
        assert_eq!(summary["failed"], 1);
        assert_eq!(summary["service_cold_ms"]["samples"], 1);
        assert_eq!(summary["service_cold_ms"]["p50"], 100.0);
        assert_eq!(summary["success_rate"], 0.5);
    }
    #[test]
    fn alternating_variants_have_independent_distributions_and_percentage_changes() {
        let summary = summarize(&[
            sample("baseline", false, "completed", 100.0),
            sample("candidate", false, "completed", 120.0),
        ]);
        assert_eq!(
            summary["variants"]["baseline"]["service_cold_ms"]["p50"],
            100.0
        );
        assert_eq!(
            summary["alternating_comparison"]["/service_cold_ms/p50"]["candidate_minus_baseline"],
            20.0
        );
        assert_eq!(
            summary["alternating_comparison"]["/service_cold_ms/p50"]["change_percent"],
            20.0
        );
    }
    #[test]
    fn degraded_capabilities_do_not_make_main_latency_look_faster() {
        let mut degraded = sample("candidate", false, "completed", 1.0);
        degraded.measurement = Some(super::super::probe::Measurement {
            configuration: Value::Null,
            configuration_fingerprint: String::new(),
            phases_ms: BTreeMap::new(),
            timings: Default::default(),
            first_text_ms: Some(1.0),
            turn_ms: None,
            peak_rss_bytes: None,
            cpu_seconds: None,
            ready_scope: String::new(),
            warmup_turn_ms: None,
            restore_ms: None,
            capabilities_evaluated: true,
            degraded: vec!["connector unavailable".into()],
        });
        let summary = summarize(&[degraded, sample("candidate", false, "completed", 100.0)]);
        assert_eq!(summary["successful"], 2);
        assert_eq!(summary["degraded"], 1);
        assert_eq!(summary["latency_samples"], 1);
        assert_eq!(summary["service_cold_ms"]["p50"], 100.0);
        assert_eq!(summary["degraded_turn_to_first_text_ms"]["p50"], 1.0);
    }
}
