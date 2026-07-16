use metered::entry::metric;
use metered::{DynamicExponentialHistogram, FixedExponentialHistogram, Registry};
use metered_om::{HistogramProfile, OpenMetricsEncoder};

fn render_vmrange() -> Result<String, std::fmt::Error> {
    let fixed = FixedExponentialHistogram::new(0.001, 10.0, 4);
    let dynamic = DynamicExponentialHistogram::with_params(5, 256);

    for value in [0.002, 0.05, 0.4, 3.0] {
        fixed.observe(value);
        dynamic.observe(value);
    }

    let mut registry = Registry::new();
    registry.register(
        metric("fixed_seconds")
            .source(&fixed)
            .help("Fixed exponential latency"),
    );
    registry.register(
        metric("dynamic_seconds")
            .source(&dynamic)
            .help("Dynamic exponential latency"),
    );

    let mut text = String::new();
    {
        let mut encoder =
            OpenMetricsEncoder::new(&mut text).histogram_profile(HistogramProfile::VmRange);
        registry.encode(&mut encoder)?;
        encoder.finish()?;
    }
    Ok(text)
}
