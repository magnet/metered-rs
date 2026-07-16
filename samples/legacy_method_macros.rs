// Compatibility-only sample for the old method macro model.
//
// Requires:
//
// metered-semantic = "0.10"
//
// New code should prefer explicit metric trees/views and tracing-derived span
// metrics unless it is maintaining an existing macro-instrumented API.

mod sample {
    use metered_semantic::{metered, Elapsed, HitCount};

    #[derive(Default)]
    struct Api {
        metrics: ApiMetrics,
    }

    #[metered(registry = ApiMetrics)]
    impl Api {
        #[measure([HitCount, Elapsed])]
        #[metric(rename = "request")]
        fn handle_request(&self) -> &'static str {
            "ok"
        }
    }
}
