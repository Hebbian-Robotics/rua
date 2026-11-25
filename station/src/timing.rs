use hdrhistogram::Histogram;
use hebbian_rua_core::control_loop::Report;
use hebbian_rua_core::so101::driver::BusTiming;

fn millisecond_percentiles(
    microseconds: &Histogram<u64>,
) -> serde_json::Map<String, serde_json::Value> {
    let milliseconds = |value: u64| std::time::Duration::from_micros(value).as_secs_f64() * 1000.0;
    if microseconds.is_empty() {
        return serde_json::Map::new();
    }
    [
        ("median_ms", microseconds.value_at_quantile(0.5)),
        ("p99_ms", microseconds.value_at_quantile(0.99)),
        ("p999_ms", microseconds.value_at_quantile(0.999)),
        ("max_ms", microseconds.max()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), milliseconds(value).into()))
    .collect()
}

pub(crate) fn bus_timing_json(timing: &BusTiming) -> serde_json::Value {
    let mut summary = millisecond_percentiles(&timing.answered_round_trip_microseconds);
    summary.insert(
        "answered".to_owned(),
        timing.answered_round_trip_microseconds.len().into(),
    );
    summary.insert("retries".to_owned(), timing.retries.into());
    summary.insert("failed_requests".to_owned(), timing.failed_requests.into());
    serde_json::Value::Object(summary)
}

pub(crate) fn print_timing(arm: &str, report: &Report, bus: Option<&BusTiming>) {
    let mut timing = millisecond_percentiles(&report.cycle_microseconds);
    timing.insert("arm".to_owned(), arm.into());
    timing.insert("cycles".to_owned(), report.cycle_microseconds.len().into());
    timing.insert("overruns".to_owned(), report.overruns.into());
    timing.insert("tripped".to_owned(), report.trip.clone().into());
    if let Some(bus) = bus {
        timing.insert("bus".to_owned(), bus_timing_json(bus));
    }
    if let Some(trace) = &report.trace {
        timing.insert(
            "trace".to_owned(),
            serde_json::json!({
                "path": trace.path,
                "written": trace.written,
                "dropped": trace.dropped,
                "error": trace.error,
            }),
        );
    }
    println!("{}", serde_json::Value::Object(timing));
}
