use std::path::Path;

/// Read-increment-write `.phronesis/journey/seq` under flock; return the new
/// value. The seq drives the `c` (last-N-calls) windows the rules can ask
/// for, so it must monotonically rise across concurrent hook processes.
///
/// Best-effort: any IO error returns 0. The journal still appends; ordering
/// degrades gracefully when many calls share seq=0 (call-window aggregators
/// use record position, not seq, for windowing). The seq is mostly a debug
/// aid in v1.
pub(super) fn next_seq(project_root: &Path) -> u64 {
    let dir = project_root.join(".phronesis").join("journey");
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    bump_seq_file(&dir.join("seq")).unwrap_or(0)
}

/// Open `path` (creating if absent), flock it exclusively, read the current
/// counter, increment by one, write back, unlock, and return the new value.
fn bump_seq_file(path: &Path) -> std::io::Result<u64> {
    use fs2::FileExt;
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock_exclusive()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok();
    let current: u64 = buf.trim().parse().unwrap_or(0);
    let next = current + 1;
    file.seek(SeekFrom::Start(0)).ok();
    file.set_len(0).ok();
    file.write_all(next.to_string().as_bytes()).ok();
    FileExt::unlock(&file).ok();
    Ok(next)
}
