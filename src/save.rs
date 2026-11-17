use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

use crate::har::{ext_for_mime, Entry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutLayout {
    /// dist/<host>/<url path>
    Tree,
    /// every file directly in dist/
    Flat,
    /// dist/<type>/<file>
    Type,
}

impl OutLayout {
    pub fn next(self) -> Self {
        match self {
            OutLayout::Tree => OutLayout::Flat,
            OutLayout::Flat => OutLayout::Type,
            OutLayout::Type => OutLayout::Tree,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutLayout::Tree => "tree (host/path)",
            OutLayout::Flat => "flat",
            OutLayout::Type => "by type",
        }
    }
}

#[derive(Default)]
pub struct Report {
    pub saved: usize,
    /// No body recorded in the HAR.
    pub skipped: usize,
    /// Same URL appeared more than once; saved only the first.
    pub duplicates: usize,
    pub errors: Vec<String>,
}

pub fn save(entries: &[&Entry], out: &Path, layout: OutLayout) -> Report {
    let mut report = Report::default();
    let mut used_paths = HashSet::new();
    let mut seen_urls = HashSet::new();

    for e in entries {
        let body = match e.body() {
            None => {
                report.skipped += 1;
                continue;
            }
            Some(Err(err)) => {
                report.errors.push(format!("{}: {err}", e.display_url()));
                continue;
            }
            Some(Ok(b)) => b,
        };
        if !seen_urls.insert(e.url.as_str()) {
            report.duplicates += 1;
            continue;
        }
        let path = unique(target(e, out, layout), &mut used_paths);
        if let Some(dir) = path.parent() {
            if let Err(err) = fs::create_dir_all(dir) {
                report.errors.push(format!("{}: {err}", dir.display()));
                continue;
            }
        }
        match fs::write(&path, body) {
            Ok(()) => report.saved += 1,
            Err(err) => report.errors.push(format!("{}: {err}", path.display())),
        }
    }
    report
}

fn sanitize(seg: &str) -> String {
    let s: String = seg
        .chars()
        .map(|c| if c.is_control() || r#"<>:"\|?*"#.contains(c) { '_' } else { c })
        .take(150)
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() || s == "." || s == ".." { "_".into() } else { s }
}

fn target(e: &Entry, out: &Path, layout: OutLayout) -> PathBuf {
    let mut segs: Vec<String> = e.path.split('/').filter(|s| !s.is_empty()).map(sanitize).collect();
    if segs.is_empty() || e.path.ends_with('/') {
        segs.push("index".into());
    }
    let mut file = segs.pop().unwrap();
    if Path::new(&file).extension().is_none() {
        if let Some(ext) = ext_for_mime(&e.mime) {
            file = format!("{file}.{ext}");
        }
    }

    let mut p = out.to_path_buf();
    match layout {
        OutLayout::Tree => {
            p.push(sanitize(&e.host.replace(':', "_")));
            p.extend(segs);
        }
        OutLayout::Flat => {}
        OutLayout::Type => p.push(e.category),
    }
    p.push(file);
    p
}

/// Same target twice in one run (e.g. same path, different query string) -> name-1.ext, name-2.ext...
fn unique(p: PathBuf, used: &mut HashSet<PathBuf>) -> PathBuf {
    if used.insert(p.clone()) {
        return p;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|i| p.with_file_name(format!("{stem}-{i}{ext}")))
        .find(|c| used.insert(c.clone()))
        .unwrap()
}
