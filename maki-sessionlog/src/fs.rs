use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, io, thread};

const LOG_EXTENSION: &str = "jsonl";
const ARCHIVE_DIR: &str = "archive";

/// maki links every session into `by-path/` as well, so one log is usually
/// reachable twice. The deeper, descriptive name wins; dangling links are not logs.
pub fn collect_logs(roots: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for root in roots {
        if root.is_dir() {
            walk(root, &mut found)?;
        } else {
            found.push(root.clone());
        }
    }

    let mut unique: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for path in found {
        let Ok(real) = path.canonicalize() else {
            continue;
        };
        match unique.entry(real) {
            Entry::Vacant(slot) => {
                slot.insert(path);
            }
            Entry::Occupied(mut slot) => {
                if path.components().count() > slot.get().components().count() {
                    slot.insert(path);
                }
            }
        }
    }
    let mut logs: Vec<PathBuf> = unique.into_values().collect();
    logs.sort();
    Ok(logs)
}

fn walk(dir: &Path, logs: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_none_or(|name| name != ARCHIVE_DIR) {
                walk(&path, logs)?;
            }
        } else if path.extension().is_some_and(|ext| ext == LOG_EXTENSION) {
            logs.push(path);
        }
    }
    Ok(())
}

/// Results keep the order of `items`, so output stays deterministic however
/// the workers interleave.
pub fn par_map<T, R>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(items.len()));
    let workers = thread::available_parallelism().map_or(1, usize::from);
    thread::scope(|scope| {
        for _ in 0..workers.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    let result = work(item);
                    results.lock().unwrap().push((index, result));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_finds_logs_and_skips_archives() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("by-path/proj");
        let archive = dir.path().join(ARCHIVE_DIR);
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&archive).unwrap();
        fs::write(nested.join("a.jsonl"), "").unwrap();
        fs::write(dir.path().join("b.jsonl"), "").unwrap();
        fs::write(dir.path().join("note.txt"), "").unwrap();
        fs::write(archive.join("old.jsonl"), "").unwrap();

        let logs = collect_logs(&[dir.path().to_path_buf()]).unwrap();
        let relative: Vec<_> = logs
            .iter()
            .map(|path| path.strip_prefix(dir.path()).unwrap().to_str().unwrap())
            .collect();
        assert_eq!(relative, ["b.jsonl", "by-path/proj/a.jsonl"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_duplicates_collapse_to_the_deeper_path_and_dangling_links_vanish() {
        let dir = tempfile::tempdir().unwrap();
        let by_path = dir.path().join("by-path/proj");
        fs::create_dir_all(&by_path).unwrap();
        let real = dir.path().join("abc.jsonl");
        fs::write(&real, "").unwrap();
        std::os::unix::fs::symlink(&real, by_path.join("2026-proj.jsonl")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), by_path.join("dangling.jsonl"))
            .unwrap();

        let logs = collect_logs(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(logs, [by_path.join("2026-proj.jsonl")]);
    }

    #[test]
    fn par_map_preserves_input_order() {
        let items: Vec<usize> = (0..200).collect();
        let doubled = par_map(&items, |n| n * 2);
        assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }
}
