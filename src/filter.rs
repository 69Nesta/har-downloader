use regex::{Regex, RegexBuilder};

use crate::har::Entry;

#[derive(Default)]
pub struct Filters {
    /// Categories (image, script...) or extensions (png, js...). Empty = all.
    pub types: Vec<String>,
    /// Hosts; `example.com` also matches `cdn.example.com`. Empty = all.
    pub domains: Vec<String>,
    query: String,
    regex: Option<Regex>,
    pub regex_error: bool,
    /// Comma separated patterns, `x` is a wildcard: `200`, `2xx,304`.
    pub status: String,
    pub min_size: usize,
    /// Also show entries whose body was not recorded in the HAR.
    pub include_empty: bool,
}

impl Filters {
    pub fn query(&self) -> &str {
        &self.query
    }

    /// URL filter: a case-insensitive regex, or plain substring if the regex is invalid.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.regex = None;
        self.regex_error = false;
        if !q.is_empty() {
            match RegexBuilder::new(q).case_insensitive(true).build() {
                Ok(r) => self.regex = Some(r),
                Err(_) => self.regex_error = true,
            }
        }
    }

    pub fn reset(&mut self) {
        let include_empty = self.include_empty;
        *self = Filters::default();
        self.include_empty = include_empty;
    }

    pub fn matches(&self, e: &Entry) -> bool {
        if !self.include_empty && !e.has_body() {
            return false;
        }
        if !self.types.is_empty()
            && !self.types.iter().any(|t| t == e.category || *t == e.ext)
        {
            return false;
        }
        if !self.domains.is_empty()
            && !self
                .domains
                .iter()
                .any(|d| e.host == *d || e.host.ends_with(&format!(".{d}")))
        {
            return false;
        }
        if !self.query.is_empty() {
            let ok = match &self.regex {
                Some(r) => r.is_match(&e.url),
                None => e.url.to_lowercase().contains(&self.query.to_lowercase()),
            };
            if !ok {
                return false;
            }
        }
        if !status_matches(&self.status, e.status) {
            return false;
        }
        e.size >= self.min_size
    }
}

pub fn valid_status_pattern(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, 'x' | 'X' | ',' | ' '))
}

fn status_matches(pattern: &str, status: i64) -> bool {
    let s = status.to_string();
    let mut pats = pattern.split(',').map(str::trim).filter(|p| !p.is_empty()).peekable();
    if pats.peek().is_none() {
        return true;
    }
    pats.any(|p| {
        p.len() == s.len()
            && p.chars()
                .zip(s.chars())
                .all(|(a, b)| a == 'x' || a == 'X' || a == b)
    })
}
