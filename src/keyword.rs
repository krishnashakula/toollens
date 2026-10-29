//! BM25 over tool names, descriptions and parameter names.

use std::collections::HashMap;

use crate::{catalog::ToolRecord, text::tokens};

const K1: f32 = 1.2;
const B: f32 = 0.75;
/// Name tokens are repeated so a word in the tool name outweighs the same word in prose.
const NAME_BOOST: usize = 3;

pub struct Bm25 {
    tf: Vec<HashMap<String, f32>>,
    len: Vec<f32>,
    avg_len: f32,
    idf: HashMap<String, f32>,
}

pub fn document(tool: &ToolRecord) -> Vec<String> {
    let mut doc = vec![];
    for _ in 0..NAME_BOOST {
        doc.extend(tokens(&tool.name));
    }
    doc.extend(tokens(&tool.server));
    doc.extend(tokens(&tool.description));
    for (name, desc, _) in tool.params() {
        doc.extend(tokens(name));
        doc.extend(tokens(desc));
    }
    doc
}

impl Bm25 {
    pub fn build(docs: impl IntoIterator<Item = Vec<String>>) -> Self {
        let mut tf = vec![];
        let mut len = vec![];
        let mut df: HashMap<String, usize> = HashMap::new();
        for doc in docs {
            let mut counts: HashMap<String, f32> = HashMap::new();
            for t in &doc {
                *counts.entry(t.clone()).or_default() += 1.0;
            }
            for t in counts.keys() {
                *df.entry(t.clone()).or_default() += 1;
            }
            len.push(doc.len() as f32);
            tf.push(counts);
        }
        let n = tf.len() as f32;
        let avg_len = if tf.is_empty() {
            1.0
        } else {
            len.iter().sum::<f32>() / n
        };
        let idf = df
            .into_iter()
            .map(|(t, d)| (t, ((n - d as f32 + 0.5) / (d as f32 + 0.5) + 1.0).ln()))
            .collect();
        Self {
            tf,
            len,
            avg_len,
            idf,
        }
    }

    /// One score per document, in document order; 0 for no overlap.
    pub fn scores(&self, query: &[String]) -> Vec<f32> {
        let mut q: Vec<&String> = query.iter().collect();
        q.sort();
        q.dedup();
        (0..self.tf.len())
            .map(|i| {
                q.iter()
                    .filter_map(|t| {
                        let f = *self.tf[i].get(*t)?;
                        let idf = self.idf[*t];
                        Some(
                            idf * f * (K1 + 1.0)
                                / (f + K1 * (1.0 - B + B * self.len[i] / self.avg_len)),
                        )
                    })
                    .sum()
            })
            .collect()
    }

    /// Query tokens that occur in document `i`, for explaining a match.
    pub fn matched<'q>(&self, i: usize, query: &'q [String]) -> Vec<&'q str> {
        let mut m: Vec<&str> = query
            .iter()
            .filter(|t| self.tf[i].contains_key(*t))
            .map(String::as_str)
            .collect();
        m.dedup();
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_name_matches_first_and_handles_empty() {
        let docs = vec![
            tokens("list deployments for a service"),
            tokens("get logs for a service"),
            tokens("send email"),
        ];
        let bm = Bm25::build(docs);
        let s = bm.scores(&tokens("recent deployments"));
        assert!(s[0] > 0.0 && s[1] == 0.0 && s[2] == 0.0);
        assert!(bm.scores(&[]).iter().all(|x| *x == 0.0));
        assert!(
            Bm25::build(Vec::<Vec<String>>::new())
                .scores(&tokens("x"))
                .is_empty()
        );
    }
}
