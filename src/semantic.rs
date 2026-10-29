//! Embedding retrieval via a local Ollama server. Vectors are cached in SQLite by
//! (model, text) so restarts and catalog growth only embed what is new.

use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::{catalog::fnv1a, store::Store};

const BATCH: usize = 64;

pub struct Embedder {
    http: reqwest::Client,
    url: String,
    pub model: String,
}

impl Embedder {
    /// `OLLAMA_HOST` (default http://localhost:11434), `TOOLLENS_EMBED_MODEL` (default nomic-embed-text).
    pub fn from_env() -> Self {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
        let host = if host.starts_with("http") {
            host
        } else {
            format!("http://{host}")
        };
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client"),
            url: format!("{}/api/embed", host.trim_end_matches('/')),
            model: std::env::var("TOOLLENS_EMBED_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".into()),
        }
    }

    /// nomic-embed-text is trained with task prefixes; other models ignore them harmlessly.
    /// Truncated: real tool descriptions can be whole API references, longer than the
    /// embedding model's context, and one over-long input fails the whole batch.
    pub fn doc_text(name: &str, description: &str) -> String {
        clip(&format!("search_document: {name}: {description}"))
    }

    pub fn query_text(goal: &str) -> String {
        clip(&format!("search_query: {goal}"))
    }

    /// One unit-normalized query embedding, never cached, so measured latency is real.
    pub async fn embed_query(&self, goal: &str) -> anyhow::Result<Vec<f32>> {
        let v = self.request(&[&Self::query_text(goal)]).await?;
        Ok(normalize(
            v.into_iter()
                .next()
                .expect("one vector per input, checked in request"),
        ))
    }

    /// Unit-normalized embeddings, one per text, using the cache where possible.
    pub async fn embed(&self, store: &Store, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let keys: Vec<String> = texts
            .iter()
            .map(|t| format!("{}:{:016x}", self.model, fnv1a(t.as_bytes())))
            .collect();
        let mut out: Vec<Option<Vec<f32>>> = keys
            .iter()
            .map(|k| store.embedding(k))
            .collect::<Result<_, _>>()?;
        let missing: Vec<usize> = (0..texts.len()).filter(|i| out[*i].is_none()).collect();
        for chunk in missing.chunks(BATCH) {
            let input: Vec<&str> = chunk.iter().map(|i| texts[*i].as_str()).collect();
            let vectors = self.request(&input).await?;
            for (i, v) in chunk.iter().zip(vectors) {
                let v = normalize(v);
                store.put_embedding(&keys[*i], &v)?;
                out[*i] = Some(v);
            }
        }
        Ok(out.into_iter().map(|v| v.expect("filled above")).collect())
    }

    async fn request(&self, input: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let resp = self
            .http
            .post(&self.url)
            .json(&json!({"model": self.model, "input": input}))
            .send()
            .await
            .with_context(|| format!("embedding request to {} (is Ollama running?)", self.url))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("embedding response was not JSON")?;
        if !status.is_success() {
            bail!(
                "embedding server returned {status}: {}",
                body["error"].as_str().unwrap_or("unknown error")
            );
        }
        let vectors: Vec<Vec<f32>> = serde_json::from_value(body["embeddings"].clone())
            .context("embedding response missing `embeddings`")?;
        if vectors.len() != input.len() {
            bail!(
                "embedding server returned {} vectors for {} inputs",
                vectors.len(),
                input.len()
            );
        }
        Ok(vectors)
    }
}

/// ~500 tokens: well inside a 2,048-token embedding context, and the opening of a tool
/// description carries what the tool is for.
const MAX_EMBED_CHARS: usize = 2000;

fn clip(text: &str) -> String {
    text.chars().take(MAX_EMBED_CHARS).collect()
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// Cosine similarity of unit vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_inputs_are_clipped_on_char_boundaries() {
        let long = "é".repeat(10_000);
        let doc = Embedder::doc_text("twilio/ListCall", &long);
        assert_eq!(doc.chars().count(), MAX_EMBED_CHARS);
        assert!(doc.starts_with("search_document: twilio/ListCall: "));
        assert_eq!(Embedder::query_text("short"), "search_query: short");
    }

    #[test]
    fn normalize_handles_zero_and_scales() {
        assert_eq!(normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
        let v = normalize(vec![3.0, 4.0]);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
    }
}
