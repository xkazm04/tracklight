//! Configured billing webhook sources, keyed by provider, built from env. Held in the API state so
//! the webhook handler can look a provider up by its URL path segment.

use std::collections::HashMap;

use crate::polar::PolarSource;
use crate::stripe::StripeSource;
use crate::BillingSource;

#[derive(Default)]
pub struct BillingRegistry {
    sources: HashMap<&'static str, Box<dyn BillingSource>>,
}

impl BillingRegistry {
    /// Build from env. A provider is enabled when its webhook secret is set:
    /// `LIGHTTRACK_STRIPE_WEBHOOK_SECRET`, `LIGHTTRACK_POLAR_WEBHOOK_SECRET`.
    pub fn from_env() -> Self {
        let mut sources: HashMap<&'static str, Box<dyn BillingSource>> = HashMap::new();
        if let Some(secret) = non_empty_env("LIGHTTRACK_STRIPE_WEBHOOK_SECRET") {
            sources.insert("stripe", Box::new(StripeSource::new(secret)));
        }
        if let Some(secret) = non_empty_env("LIGHTTRACK_POLAR_WEBHOOK_SECRET") {
            // Apps key margin on their internal user id, echoed into Polar order `metadata.userId`;
            // override the key with `LIGHTTRACK_POLAR_CUSTOMER_META_KEY` if an app uses a different one.
            let source = match non_empty_env("LIGHTTRACK_POLAR_CUSTOMER_META_KEY") {
                Some(key) => PolarSource::with_customer_key(secret, key),
                None => PolarSource::new(secret),
            };
            sources.insert("polar", Box::new(source));
        }
        Self { sources }
    }

    pub fn get(&self, provider: &str) -> Option<&dyn BillingSource> {
        self.sources.get(provider).map(Box::as_ref)
    }

    /// Comma-free summary of configured providers, for the startup log.
    pub fn describe(&self) -> String {
        if self.sources.is_empty() {
            return "none".to_string();
        }
        let mut keys: Vec<&str> = self.sources.keys().copied().collect();
        keys.sort_unstable();
        keys.join("+")
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}
