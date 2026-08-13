//! Line diff engine: Myers O(ND) in linear space (middle-snake
//! divide-and-conquer), plus git's unified renderer.
//!
//! The engine emits an edit script as `Op`s; `diff_lines` trims the common
//! prefix/suffix, runs Myers on the middle, then groups the ops into hunks
//! with git's context rules (3 context lines, changes within 6 unchanged
//! lines merge into one hunk). The renderer produces `git diff` output
//! byte-for-byte: `diff --git` header, index/mode lines, `---`/`+++` lines,
//! `@@ -s,c +s,c @@` headers, and `\ No newline at end of file` markers.
//!
//! The middle-snake search follows Myers 1986 (section 4b): forward and
//! backward frontier scans on the same diagonal coordinates (`k` forward,
//! `c = k - delta` backward), first overlap wins. The forward scan iterates
//! `k` descending, like git's xdiff. Within runs of identical lines the
//! script choice is ambiguous and git additionally slides change groups
//! (`xdl_change_compact`); we do not, so hunk boundaries on all-identical
//! runs can differ from git 2.55's — see decisions.md D-014. Hunk content
//! (which lines changed) is never affected.

/// One line of a rendered hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Context(Vec<u8>),
    Delete(Vec<u8>),
    Add(Vec<u8>),
}

/// A unified-diff hunk with 1-based header positions (0 when the range is
/// empty, matching git's `s,0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
    /// git's `@@ ... @@ <funcname>` suffix: nearest preceding line starting
    /// with an ASCII letter, `_` or `$` (def_ff). Sticky across hunks.
    pub funcname: Vec<u8>,
}

/// One edit-script op: `(old index, new index, count)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `count` lines equal in both files.
    Equal(usize, usize, usize),
    /// `count` lines deleted from old (new cursor stays put).
    Delete(usize, usize, usize),
    /// `count` lines inserted into new (old cursor stays put).
    Insert(usize, usize, usize),
}

/// Split file content into lines, keeping the trailing `\n` in each line.
/// A file without a final newline yields a last line without one — the
/// renderer flags those with `\ No newline at end of file`.
pub fn split_lines(content: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            out.push(content[start..=i].to_vec());
            start = i + 1;
        }
    }
    if start < content.len() {
        out.push(content[start..].to_vec());
    }
    out
}

/// True when the content should be treated as binary: a NUL byte in the
/// first 8000 bytes (git's rule).
pub fn is_binary(content: &[u8]) -> bool {
    content[..content.len().min(8000)].contains(&0)
}

/// Diff two line arrays into hunks (context lines already expanded).
pub fn diff_lines(old: &[Vec<u8>], new: &[Vec<u8>]) -> Vec<Hunk> {
    let mut ops = Vec::new();
    let mut pre = 0;
    while pre < old.len() && pre < new.len() && old[pre] == new[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < old.len() - pre
        && suf < new.len() - pre
        && old[old.len() - 1 - suf] == new[new.len() - 1 - suf]
    {
        suf += 1;
    }
    if pre > 0 {
        ops.push(Op::Equal(0, 0, pre));
    }
    let box_ = Box {
        left: pre,
        top: pre,
        right: old.len() - suf,
        bottom: new.len() - suf,
    };
    if box_.size() > 0 {
        find_path(&box_, old, new, &mut ops);
    }
    if suf > 0 {
        ops.push(Op::Equal(old.len() - suf, new.len() - suf, suf));
    }
    group_hunks(old, new, &ops)
}

/// One edit-graph rectangle `[left, right) x [top, bottom)`.
#[derive(Debug, Clone, Copy)]
struct Box {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Box {
    fn width(&self) -> usize {
        self.right - self.left
    }
    fn height(&self) -> usize {
        self.bottom - self.top
    }
    fn size(&self) -> usize {
        self.width() + self.height()
    }
    /// Diagonal offset of the bottom-right corner from the top-left.
    fn delta(&self) -> isize {
        self.width() as isize - self.height() as isize
    }
}

/// Emit the edit path through `b` by repeatedly splitting at the middle
/// snake. Iterative (explicit stack) so the recursion depth can't overflow
/// on fully-different files.
fn find_path(b: &Box, old: &[Vec<u8>], new: &[Vec<u8>], out: &mut Vec<Op>) {
    enum Task {
        Box(Box),
        Snake(usize, usize, usize, usize),
    }
    let mut stack = vec![Task::Box(*b)];
    while let Some(task) = stack.pop() {
        match task {
            Task::Snake(sx, sy, ex, ey) => emit_snake(sx, sy, ex, ey, old, new, out),
            Task::Box(cur) => {
                let Some((sx, sy, ex, ey)) = midpoint(&cur, old, new) else {
                    continue;
                };
                let head = Box {
                    left: cur.left,
                    top: cur.top,
                    right: sx,
                    bottom: sy,
                };
                let tail = Box {
                    left: ex,
                    top: ey,
                    right: cur.right,
                    bottom: cur.bottom,
                };
                debug_assert!(
                    head.right >= head.left && head.bottom >= head.top,
                    "snake start outside its box"
                );
                debug_assert!(
                    tail.right >= tail.left && tail.bottom >= tail.top,
                    "snake end outside its box"
                );
                debug_assert!(head.size() < cur.size() && tail.size() < cur.size());
                stack.push(Task::Box(tail));
                stack.push(Task::Snake(sx, sy, ex, ey));
                stack.push(Task::Box(head));
            }
        }
    }
}

/// The middle snake of `b` as `(start_x, start_y, end_x, end_y)`, or `None`
/// when the box is a single point. Frontier arrays are indexed by
/// `k + max` (and `c + max` for the backward scan); slot 1 holds the
/// d=0 sentinel.
fn midpoint(b: &Box, old: &[Vec<u8>], new: &[Vec<u8>]) -> Option<(usize, usize, usize, usize)> {
    if b.size() == 0 {
        return None;
    }
    let max = b.size().div_ceil(2);
    let mut vf = vec![0isize; 2 * max + 1];
    let mut vb = vec![0isize; 2 * max + 1];
    // d=0 sentinels: the d=0 scans read `k+1 = 1`, which is slot `max + 1`.
    vf[max + 1] = b.left as isize;
    vb[max + 1] = b.bottom as isize;
    for d in 0..=max {
        if let Some(snake) = scan_forward(b, &mut vf, &vb, d, old, new) {
            return Some(snake);
        }
        if let Some(snake) = scan_backward(b, &vf, &mut vb, d, old, new) {
            return Some(snake);
        }
    }
    None
}

/// One forward iteration of the greedy search from the box's top-left.
fn scan_forward(
    b: &Box,
    vf: &mut [isize],
    vb: &[isize],
    d: usize,
    old: &[Vec<u8>],
    new: &[Vec<u8>],
) -> Option<(usize, usize, usize, usize)> {
    let d = d as isize;
    let max = b.size().div_ceil(2);
    let delta = b.delta();
    let mut k = d;
    while k >= -d {
        let c = k - delta;
        let (mut x, px) = if k == -d || (k != d && vf[at(k - 1, max)] < vf[at(k + 1, max)]) {
            let x = vf[at(k + 1, max)];
            (x, x)
        } else {
            let px = vf[at(k - 1, max)];
            (px + 1, px)
        };
        let mut y = b.top as isize + (x - b.left as isize) - k;
        let py = if d == 0 || x != px { y } else { y - 1 };
        while x < b.right as isize && y < b.bottom as isize && old[x as usize] == new[y as usize] {
            x += 1;
            y += 1;
        }
        vf[at(k, max)] = x;
        if delta % 2 != 0 && c >= -(d - 1) && c < d && y >= vb[at(c, max)] {
            return Some((px as usize, py as usize, x as usize, y as usize));
        }
        k -= 2;
    }
    None
}

/// One backward iteration of the greedy search from the box's bottom-right.
fn scan_backward(
    b: &Box,
    vf: &[isize],
    vb: &mut [isize],
    d: usize,
    old: &[Vec<u8>],
    new: &[Vec<u8>],
) -> Option<(usize, usize, usize, usize)> {
    let d = d as isize;
    let max = b.size().div_ceil(2);
    let delta = b.delta();
    let mut c = d;
    while c >= -d {
        let k = c + delta;
        let (mut y, py) = if c == -d || (c != d && vb[at(c - 1, max)] > vb[at(c + 1, max)]) {
            let y = vb[at(c + 1, max)];
            (y, y)
        } else {
            let py = vb[at(c - 1, max)];
            (py - 1, py)
        };
        let mut x = b.left as isize + (y - b.top as isize) + k;
        let px = if d == 0 || y != py { x } else { x + 1 };
        while x > b.left as isize
            && y > b.top as isize
            && old[x as usize - 1] == new[y as usize - 1]
        {
            x -= 1;
            y -= 1;
        }
        vb[at(c, max)] = y;
        if delta % 2 == 0 && k >= -d && k <= d && x <= vf[at(k, max)] {
            return Some((x as usize, y as usize, px as usize, py as usize));
        }
        c -= 2;
    }
    None
}

/// Array index for frontier coordinate `k` (or `c`).
fn at(k: isize, max: usize) -> usize {
    (k + max as isize) as usize
}

/// Turn a snake into ops: a content-checked diagonal walk, one off-diagonal
/// step, then the remaining diagonal walk (jcoglan's `walk_snakes`). Whether
/// the step comes first or last depends on the content at the snake's start,
/// so the walks must be content-checked, not positional. The snake's
/// endpoints can sit one past the arrays when the split came from the
/// d=0 sentinel, so bound every walk and step by the array lengths.
fn emit_snake(
    sx: usize,
    sy: usize,
    ex: usize,
    ey: usize,
    old: &[Vec<u8>],
    new: &[Vec<u8>],
    out: &mut Vec<Op>,
) {
    let (mut x1, mut y1) = (sx, sy);
    while x1 < ex && y1 < ey && x1 < old.len() && y1 < new.len() && old[x1] == new[y1] {
        x1 += 1;
        y1 += 1;
    }
    if x1 > sx {
        out.push(Op::Equal(sx, sy, x1 - sx));
    }
    let (dx, dy) = (ex - x1, ey - y1);
    if dx > dy {
        if x1 + (dx - dy) <= old.len() && y1 <= new.len() {
            out.push(Op::Delete(x1, y1, dx - dy));
            x1 += dx - dy;
        }
    } else if dy > dx && y1 + (dy - dx) <= new.len() && x1 <= old.len() {
        out.push(Op::Insert(x1, y1, dy - dx));
        y1 += dy - dx;
    }
    let (s2x, s2y) = (x1, y1);
    while x1 < ex && y1 < ey && x1 < old.len() && y1 < new.len() && old[x1] == new[y1] {
        x1 += 1;
        y1 += 1;
    }
    if x1 > s2x {
        out.push(Op::Equal(s2x, s2y, x1 - s2x));
    }
}

/// Context lines on each side of a change.
const CTX: usize = 3;

/// Merge change runs (gap of unchanged lines <= 2*CTX) and expand each into
/// a hunk with CTX context lines, exactly like git's xdl_emit_diff.
fn group_hunks(old: &[Vec<u8>], new: &[Vec<u8>], ops: &[Op]) -> Vec<Hunk> {
    #[derive(Debug, Clone, Copy)]
    struct Change {
        old_min: usize,
        old_max: usize,
        new_min: usize,
        new_max: usize,
    }
    // Flatten ops into positioned lines; track maximal change runs.
    let mut flat: Vec<(usize, usize, DiffLine)> = Vec::new();
    let mut changes: Vec<Change> = Vec::new();
    let mut cur: Option<Change> = None;
    for op in ops {
        match *op {
            Op::Equal(o, n, len) => {
                for i in 0..len {
                    flat.push((o + i, n + i, DiffLine::Context(old[o + i].clone())));
                }
                if let Some(c) = cur.take() {
                    changes.push(c);
                }
            }
            Op::Delete(o, n, len) => {
                for i in 0..len {
                    flat.push((o + i, n, DiffLine::Delete(old[o + i].clone())));
                }
                let c = cur.get_or_insert(Change {
                    old_min: o,
                    old_max: o + len,
                    new_min: n,
                    new_max: n,
                });
                c.old_min = c.old_min.min(o);
                c.old_max = c.old_max.max(o + len);
                c.new_min = c.new_min.min(n);
                c.new_max = c.new_max.max(n + 1);
            }
            Op::Insert(o, n, len) => {
                for i in 0..len {
                    flat.push((o, n + i, DiffLine::Add(new[n + i].clone())));
                }
                let c = cur.get_or_insert(Change {
                    old_min: o,
                    old_max: o,
                    new_min: n,
                    new_max: n + len,
                });
                // An insert consumes no old lines: zero width on the old
                // side, so the merge gap is measured to the next change.
                c.old_min = c.old_min.min(o);
                c.new_min = c.new_min.min(n);
                c.new_max = c.new_max.max(n + len);
            }
        }
    }
    if let Some(c) = cur.take() {
        changes.push(c);
    }
    // Merge runs whose separating equal-run is at most 2*CTX lines.
    let mut groups: Vec<Change> = Vec::new();
    for c in changes {
        if let Some(g) = groups.last_mut()
            && c.old_min.saturating_sub(g.old_max) <= 2 * CTX
        {
            g.old_max = g.old_max.max(c.old_max);
            g.new_max = g.new_max.max(c.new_max);
            continue;
        }
        groups.push(c);
    }
    // Expand each group with context and slice the flat line list.
    let mut hunks = Vec::new();
    // git's funcname is sticky: the scan region is (previous hunk's
    // pre-context start, this one's], and misses keep the previous value.
    let mut prev_stop: isize = -1;
    let mut carried: Vec<u8> = Vec::new();
    for g in groups {
        let old_start = g.old_min.saturating_sub(CTX);
        let old_end = (g.old_max + CTX).min(old.len());
        let new_start = g.new_min.saturating_sub(CTX);
        let new_end = (g.new_max + CTX).min(new.len());
        let from = old_start as isize - 1;
        if let Some(f) = funcname_for(old, from, prev_stop) {
            carried = f;
        }
        prev_stop = from;
        let lines: Vec<DiffLine> = flat
            .iter()
            .filter(|(o, n, _)| {
                (*o >= old_start && *o < old_end) || (*n >= new_start && *n < new_end)
            })
            .map(|(_, _, l)| l.clone())
            .collect();
        let old_count = old_end - old_start;
        let new_count = new_end - new_start;
        hunks.push(Hunk {
            old_start: if old_count > 0 {
                old_start as u32 + 1
            } else {
                0
            },
            old_count: old_count as u32,
            new_start: if new_count > 0 {
                new_start as u32 + 1
            } else {
                0
            },
            new_count: new_count as u32,
            lines,
            funcname: carried.clone(),
        });
    }
    hunks
}

/// git's `xdl_find_func`/`def_ff` without a funcname pattern: scan old lines
/// from `from` down to `stop` (exclusive); the first line starting with an
/// ASCII letter, `_` or `$` wins; result is its first 80 bytes with trailing
/// whitespace trimmed. `None` = nothing qualifies (sticky handling upstream).
fn funcname_for(old: &[Vec<u8>], from: isize, stop: isize) -> Option<Vec<u8>> {
    let mut l = from;
    while l > stop && l >= 0 {
        let line = &old[l as usize];
        if !line.is_empty() {
            let b = line[0];
            if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
                let mut s = line[..line.len().min(80)].to_vec();
                while let Some(&last) = s.last() {
                    if last.is_ascii_whitespace() {
                        s.pop();
                    } else {
                        break;
                    }
                }
                return Some(s);
            }
        }
        l -= 1;
    }
    None
}

/// Everything the renderer needs for one file's `git diff` block.
pub struct FileDiff {
    /// `diff --git` / binary-line paths: `a/<path>` / `b/<path>`, quoted.
    pub hdr_old: String,
    pub hdr_new: String,
    /// `---` / `+++` paths: as above, or `/dev/null` when the side is empty.
    pub body_old: String,
    pub body_new: String,
    /// 40-hex oids; the missing side is `0000...0`.
    pub old_oid: String,
    pub new_oid: String,
    /// File modes (0 = side missing).
    pub old_mode: u32,
    pub new_mode: u32,
    /// Binary: emit `Binary files ... differ` instead of hunks.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// Render one file's diff block, byte-identical to real git's unified
/// output (probed against git 2.55).
pub fn render(f: &FileDiff) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("diff --git {} {}\n", f.hdr_old, f.hdr_new).as_bytes());
    if f.old_mode == 0 {
        out.extend_from_slice(format!("new file mode {:06o}\n", f.new_mode).as_bytes());
    } else if f.new_mode == 0 {
        out.extend_from_slice(format!("deleted file mode {:06o}\n", f.old_mode).as_bytes());
    } else if f.old_mode != f.new_mode {
        out.extend_from_slice(
            format!("old mode {:06o}\nnew mode {:06o}\n", f.old_mode, f.new_mode).as_bytes(),
        );
    }
    let mut index = format!("index {}..{}", &f.old_oid[..7], &f.new_oid[..7]);
    if f.old_mode != 0 && f.new_mode != 0 && f.old_mode == f.new_mode {
        index.push_str(&format!(" {:06o}", f.old_mode));
    }
    out.extend_from_slice(format!("{index}\n").as_bytes());
    if f.binary {
        out.extend_from_slice(
            format!("Binary files {} and {} differ\n", f.hdr_old, f.hdr_new).as_bytes(),
        );
        return out;
    }
    if !f.hunks.is_empty() {
        out.extend_from_slice(format!("--- {}\n", f.body_old).as_bytes());
        out.extend_from_slice(format!("+++ {}\n", f.body_new).as_bytes());
        for h in &f.hunks {
            let mut head = format!(
                "@@ -{} +{} @@",
                range(h.old_start, h.old_count),
                range(h.new_start, h.new_count)
            )
            .into_bytes();
            if !h.funcname.is_empty() {
                head.push(b' ');
                head.extend_from_slice(&h.funcname);
            }
            head.push(b'\n');
            out.extend_from_slice(&head);
            for line in &h.lines {
                let (prefix, content) = match line {
                    DiffLine::Context(c) => (b' ', c),
                    DiffLine::Delete(c) => (b'-', c),
                    DiffLine::Add(c) => (b'+', c),
                };
                out.push(prefix);
                out.extend_from_slice(content);
                if !content.ends_with(b"\n") {
                    out.push(b'\n');
                    out.extend_from_slice(b"\\ No newline at end of file\n");
                }
            }
        }
    }
    out
}

/// `s,c` with the `,c` omitted when the count is 1 (git omits it).
fn range(start: u32, count: u32) -> String {
    if count == 1 {
        format!("{start}")
    } else {
        format!("{start},{count}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(contents: &[&str]) -> Vec<Vec<u8>> {
        contents.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    fn op_sizes(ops: &[Op]) -> (usize, usize, usize) {
        let (mut eq, mut del, mut ins) = (0, 0, 0);
        for op in ops {
            match op {
                Op::Equal(_, _, l) => eq += l,
                Op::Delete(_, _, l) => del += l,
                Op::Insert(_, _, l) => ins += l,
            }
        }
        (eq, del, ins)
    }

    /// Edit script must be consistent and minimal.
    fn check_optimal(old: &[Vec<u8>], new: &[Vec<u8>]) {
        let mut ops = Vec::new();
        find_path(
            &Box {
                left: 0,
                top: 0,
                right: old.len(),
                bottom: new.len(),
            },
            old,
            new,
            &mut ops,
        );
        // Validity: ops walk the arrays exactly once, in order.
        let (mut o, mut n) = (0, 0);
        for op in &ops {
            match *op {
                Op::Equal(oi, ni, l) => {
                    assert_eq!(oi, o, "equal position");
                    assert_eq!(ni, n, "equal position");
                    for k in 0..l {
                        assert_eq!(old[o + k], new[n + k], "equal content");
                    }
                    o += l;
                    n += l;
                }
                Op::Delete(oi, ni, l) => {
                    assert_eq!(oi, o, "delete position");
                    assert_eq!(ni, n, "delete position");
                    o += l;
                }
                Op::Insert(oi, ni, l) => {
                    assert_eq!(oi, o, "insert position");
                    assert_eq!(ni, n, "insert position");
                    n += l;
                }
            }
        }
        assert_eq!(o, old.len());
        assert_eq!(n, new.len());
        // Minimality: matches the DP edit distance.
        let (_, del, ins) = op_sizes(&ops);
        assert_eq!(
            del + ins,
            dp_distance(old, new),
            "edit distance for {:?} vs {:?}",
            old,
            new
        );
    }

    /// Classic O(N*M) LCS edit distance, the reference for minimality.
    fn dp_distance(a: &[Vec<u8>], b: &[Vec<u8>]) -> usize {
        let (n, m) = (a.len(), b.len());
        let mut prev = vec![0usize; m + 1];
        let mut cur = vec![0usize; m + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                cur[j] = if a[i] == b[j] {
                    prev[j + 1] + 1
                } else {
                    prev[j].max(cur[j + 1])
                };
            }
            std::mem::swap(&mut prev, &mut cur);
        }
        n + m - 2 * prev[0]
    }

    /// Deterministic PRNG so failures are reproducible.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn pick(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn myers_matches_dp_on_random_inputs() {
        let mut rng = Rng(0x5EED_1234);
        let alphabet = [b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"\n".to_vec()];
        for _ in 0..2000 {
            let n = rng.pick(30);
            let m = rng.pick(30);
            let old: Vec<Vec<u8>> = (0..n)
                .map(|_| alphabet[rng.pick(alphabet.len())].clone())
                .collect();
            let new: Vec<Vec<u8>> = (0..m)
                .map(|_| alphabet[rng.pick(alphabet.len())].clone())
                .collect();
            check_optimal(&old, &new);
        }
    }

    #[test]
    fn myers_handles_larger_inputs() {
        let mut rng = Rng(42);
        for _ in 0..50 {
            let n = rng.pick(200) + 1;
            let m = rng.pick(200) + 1;
            let old: Vec<Vec<u8>> = (0..n)
                .map(|_| format!("line{}", rng.pick(5)).into_bytes())
                .collect();
            let new: Vec<Vec<u8>> = (0..m)
                .map(|_| format!("line{}", rng.pick(5)).into_bytes())
                .collect();
            check_optimal(&old, &new);
        }
    }

    #[test]
    fn single_line_replace() {
        // "a" vs "b": one delete + one insert, the classic trap case.
        let old = lines(&["a"]);
        let new = lines(&["b"]);
        check_optimal(&old, &new);
        let mut ops = Vec::new();
        find_path(
            &Box {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            &old,
            &new,
            &mut ops,
        );
        assert_eq!(ops, vec![Op::Delete(0, 0, 1), Op::Insert(1, 0, 1)]);
    }

    #[test]
    fn split_lines_keeps_newlines() {
        assert_eq!(split_lines(b""), Vec::<Vec<u8>>::new());
        assert_eq!(split_lines(b"a\nb"), lines(&["a\n", "b"]));
        assert_eq!(split_lines(b"a\nb\n"), lines(&["a\n", "b\n"]));
        assert_eq!(split_lines(b"\n"), lines(&["\n"]));
        assert_eq!(split_lines(b"abc"), lines(&["abc"]));
    }

    #[test]
    fn is_binary_detects_nul_in_first_8000() {
        assert!(!is_binary(b"plain text\n"));
        assert!(is_binary(b"a\0b"));
        let mut big = vec![b'x'; 9000];
        big[7999] = 0;
        assert!(is_binary(&big));
        let mut after = vec![b'x'; 9000];
        after[8000] = 0;
        assert!(!is_binary(&after)); // NUL past the window is text
    }

    #[test]
    fn hunks_merge_within_six_context_lines() {
        // Two edits separated by 6 unchanged lines merge into one hunk.
        let old = vec![b"x\n".to_vec(); 12];
        let mut new = old.clone();
        new[2] = b"y\n".to_vec();
        new[9] = b"z\n".to_vec();
        let hunks = diff_lines(&old, &new);
        assert_eq!(hunks.len(), 1, "6-line gap must merge");
        let h = &hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.old_count, 12);
        assert_eq!(h.lines.len(), 12 + 2);
        assert_eq!(h.lines[0], DiffLine::Context(b"x\n".to_vec()));

        // 7 unchanged lines split into two hunks. The split points are
        // D-014: on all-identical runs the change group does not slide like
        // git's xdl_change_compact (git 2.55 emits old spans 1-5/7-13 here).
        let old2 = vec![b"x\n".to_vec(); 13];
        let mut new2 = old2.clone();
        new2[2] = b"y\n".to_vec();
        new2[10] = b"z\n".to_vec();
        let hunks = diff_lines(&old2, &new2);
        assert_eq!(hunks.len(), 2, "7-line gap must split");
        assert_eq!((hunks[0].old_start, hunks[0].old_count), (1, 7));
        assert_eq!((hunks[1].old_start, hunks[1].old_count), (9, 5));
    }

    #[test]
    fn hunk_headers_omit_single_counts() {
        let old = lines(&["a\n", "b\n", "c\n"]);
        let mut new = old.clone();
        new[1] = b"B\n".to_vec();
        let hunks = diff_lines(&old, &new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);
    }

    #[test]
    fn empty_and_new_files() {
        assert!(diff_lines(&[], &[]).is_empty());
        assert!(diff_lines(&lines(&["a\n"]), &lines(&["a\n"])).is_empty());
        // Pure insertion at the start of an empty file.
        let hunks = diff_lines(&[], &lines(&["x\n", "y\n"]));
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_start, hunks[0].old_count), (0, 0));
        assert_eq!((hunks[0].new_start, hunks[0].new_count), (1, 2));
        // Pure deletion to empty.
        let hunks = diff_lines(&lines(&["x\n", "y\n"]), &[]);
        assert_eq!((hunks[0].old_start, hunks[0].old_count), (1, 2));
        assert_eq!((hunks[0].new_start, hunks[0].new_count), (0, 0));
    }

    #[test]
    fn render_marks_missing_final_newline() {
        let old = lines(&["a\n", "b"]);
        let mut new = lines(&["a\n", "b\n"]);
        let hunks = diff_lines(&old, &new);
        let f = FileDiff {
            hdr_old: "a/f.txt".into(),
            hdr_new: "b/f.txt".into(),
            body_old: "a/f.txt".into(),
            body_new: "b/f.txt".into(),
            old_oid: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            new_oid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            old_mode: 0o100644,
            new_mode: 0o100644,
            binary: false,
            hunks,
        };
        let out = String::from_utf8(render(&f)).unwrap();
        assert!(
            out.contains("-b\n\\ No newline at end of file\n+b\n"),
            "{out}"
        );
        let _ = &mut new;
    }

    #[test]
    fn render_headers_for_new_deleted_mode_change() {
        let new_file = FileDiff {
            hdr_old: "a/n.txt".into(),
            hdr_new: "b/n.txt".into(),
            body_old: "/dev/null".into(),
            body_new: "b/n.txt".into(),
            old_oid: "0000000000000000000000000000000000000000".into(),
            new_oid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            old_mode: 0,
            new_mode: 0o100644,
            binary: false,
            hunks: diff_lines(&[], &lines(&["x\n"])),
        };
        let out = String::from_utf8(render(&new_file)).unwrap();
        assert!(out.contains("new file mode 100644\n"), "{out}");
        assert!(out.contains("index 0000000..bbbbbbb\n"), "{out}");

        let mode_change = FileDiff {
            hdr_old: "a/m.txt".into(),
            hdr_new: "b/m.txt".into(),
            body_old: "a/m.txt".into(),
            body_new: "b/m.txt".into(),
            old_oid: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            new_oid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            old_mode: 0o100755,
            new_mode: 0o100644,
            binary: false,
            hunks: Vec::new(),
        };
        let out = String::from_utf8(render(&mode_change)).unwrap();
        assert!(out.contains("old mode 100755\nnew mode 100644\n"), "{out}");
        assert!(!out.contains("bbbbbbb 100755\n"), "{out}"); // no mode suffix on the index line when modes differ
    }

    #[test]
    fn render_binary_files_line() {
        let f = FileDiff {
            hdr_old: "a/x.bin".into(),
            hdr_new: "b/x.bin".into(),
            body_old: "a/x.bin".into(),
            body_new: "b/x.bin".into(),
            old_oid: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            new_oid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            old_mode: 0o100644,
            new_mode: 0o100644,
            binary: true,
            hunks: Vec::new(),
        };
        let out = String::from_utf8(render(&f)).unwrap();
        assert_eq!(
            out,
            "diff --git a/x.bin b/x.bin\nindex aaaaaaa..bbbbbbb 100644\nBinary files a/x.bin and b/x.bin differ\n"
        );
    }
}
