//! Automated measurements for the M0 gate: typing and cursor latency, cursor
//! navigation through hidden markers, a randomised editing soak test, and
//! screenshots.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::controller::{Controller, Mode};

#[derive(Debug, Clone)]
pub enum Task {
    Interactive,
    Bench {
        keys: usize,
    },
    Nav,
    Soak {
        secs: u64,
        seed: u64,
    },
    Screenshot {
        path: PathBuf,
        cursor: Option<usize>,
    },
}

pub fn start(
    task: Task,
    c: Rc<Controller>,
    window: &adw::ApplicationWindow,
    app: &adw::Application,
) {
    let (window, app) = (window.clone(), app.clone());
    match task {
        Task::Interactive => {}
        Task::Bench { keys } => after(1000, move || bench(c, app, keys)),
        Task::Nav => after(800, move || {
            nav(&c);
            app.quit();
        }),
        Task::Soak { secs, seed } => after(500, move || soak(c, app, secs, seed)),
        Task::Screenshot { path, cursor } => after(300, move || {
            if let Some(byte) = cursor {
                let text = c.text();
                let offset = text[..byte.min(text.len())].chars().count() as i32;
                c.buffer.place_cursor(&c.buffer.iter_at_offset(offset));
            }
            after(1200, move || {
                match screenshot(&window, &path) {
                    Ok(()) => println!("screenshot: {}", path.display()),
                    Err(e) => println!("screenshot failed: {e}"),
                }
                app.quit();
            });
        }),
    }
}

fn after(ms: u64, f: impl FnOnce() + 'static) {
    glib::timeout_add_local_once(Duration::from_millis(ms), f);
}

fn summary(name: &str, samples: &[Duration]) -> String {
    if samples.is_empty() {
        return format!("{name}: no samples");
    }
    let mut v: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    v.sort_by(f64::total_cmp);
    let pct = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    format!(
        "{name}: n={} p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
        v.len(),
        pct(0.5),
        pct(0.95),
        pct(0.99),
        v[v.len() - 1]
    )
}

// --- latency -------------------------------------------------------------------

fn bench(c: Rc<Controller>, app: adw::Application, keys: usize) {
    println!("note: {} KiB", c.text().len() / 1024);
    let frames: Rc<RefCell<Vec<Duration>>> = Rc::default();
    let pending: Rc<Cell<Option<Instant>>> = Rc::default();
    match c.view.frame_clock() {
        Some(clock) => {
            let frame_start: Rc<Cell<Option<Instant>>> = Rc::default();
            let s = frame_start.clone();
            clock.connect_before_paint(move |_| s.set(Some(Instant::now())));
            let (frames, pending) = (frames.clone(), pending.clone());
            clock.connect_after_paint(move |_| {
                if let Some(t) = pending.take() {
                    frames.borrow_mut().push(t.elapsed());
                    if let Some(f) = frame_start.get() {
                        FRAME_WORK.with(|w| w.borrow_mut().push(f.elapsed()));
                    }
                }
            });
        }
        None => println!("no frame clock: frame latency unavailable"),
    }

    let phases: Vec<Phase> = vec![
        ("typing mid-note", Box::new(type_char)),
        ("typing at top", Box::new(type_char)),
        (
            "cursor down",
            Box::new(|c, _| move_cursor(c, gtk::MovementStep::DisplayLines, 1)),
        ),
        (
            "cursor right",
            Box::new(|c, _| move_cursor(c, gtk::MovementStep::VisualPositions, 1)),
        ),
    ];
    let phases = Rc::new(phases);
    run_phase(c, app, phases, 0, keys, frames, pending);
}

thread_local! {
    /// Layout and paint time of frames that follow an input.
    static FRAME_WORK: RefCell<Vec<Duration>> = const { RefCell::new(Vec::new()) };
}

fn type_char(c: &Controller, i: usize) {
    let ch = if i % 6 == 5 { " " } else { "a" };
    c.buffer.begin_user_action();
    c.buffer.insert_at_cursor(ch);
    c.buffer.end_user_action();
}

fn move_cursor(c: &Controller, step: gtk::MovementStep, count: i32) {
    c.view
        .emit_by_name::<()>("move-cursor", &[&step, &count, &false]);
}

type Phase = (&'static str, Box<dyn Fn(&Controller, usize)>);

fn run_phase(
    c: Rc<Controller>,
    app: adw::Application,
    phases: Rc<Vec<Phase>>,
    index: usize,
    count: usize,
    frames: Rc<RefCell<Vec<Duration>>>,
    pending: Rc<Cell<Option<Instant>>>,
) {
    let Some(name) = phases.get(index).map(|(name, _)| *name) else {
        app.quit();
        return;
    };
    // Position the cursor for the phase.
    let text = c.text();
    let byte = match name {
        "typing at top" => body_start(&text) + 2,
        _ => {
            let mid = text.len() / 2;
            text[mid..]
                .find("\n\nParagraph")
                .map_or(mid, |i| mid + i + 12)
        }
    };
    let offset = text[..byte].chars().count() as i32;
    c.buffer.place_cursor(&c.buffer.iter_at_offset(offset));
    c.clear_stats();
    frames.borrow_mut().clear();
    FRAME_WORK.with(|w| w.borrow_mut().clear());

    let done = Rc::new(Cell::new(0usize));
    glib::timeout_add_local(Duration::from_millis(30), move || {
        let i = done.get();
        if i >= count {
            println!("== {name}");
            c.with_stats(|s| {
                println!("  {}", summary("restyle (per change)", &s.restyle));
                println!("  {}", summary("reveal (per cursor move)", &s.reveal));
                println!(
                    "  overlay moves: {}, attaches: {}",
                    s.overlay_moves.get(),
                    s.overlay_attach.get()
                );
            });
            println!("  {}", summary("input→frame", &frames.borrow()));
            FRAME_WORK
                .with(|w| println!("  {}", summary("frame work (layout+paint)", &w.borrow())));
            if let Err(e) = c.check_invariants() {
                println!("  INVARIANT FAILED: {e}");
            }
            run_phase(
                c.clone(),
                app.clone(),
                phases.clone(),
                index + 1,
                count,
                frames.clone(),
                pending.clone(),
            );
            return glib::ControlFlow::Break;
        }
        pending.set(Some(Instant::now()));
        (phases[index].1)(&c, i);
        done.set(i + 1);
        glib::ControlFlow::Continue
    });
}

fn body_start(text: &str) -> usize {
    igneous_markdown::frontmatter::detect(text).map_or(0, |(r, _)| r.end)
}

// --- navigation ----------------------------------------------------------------

fn cursor_offset(c: &Controller) -> i32 {
    c.buffer.iter_at_mark(&c.buffer.get_insert()).offset()
}

fn check_task_toggle(c: &Controller) {
    let Some(check) = c.first_checkbox() else {
        println!("== task toggle: no checkbox on screen");
        return;
    };
    let before = c.text();
    let was = check.is_active();
    check.set_active(!was);
    let after = c.text();
    let changed: Vec<(char, char)> = before
        .chars()
        .zip(after.chars())
        .filter(|(a, b)| a != b)
        .collect();
    c.buffer.undo();
    let undone = c.text() == before;
    println!(
        "== task toggle: {} -> {}, text changes {:?}, same length: {}, undo restores: {undone}",
        was,
        !was,
        changed,
        before.len() == after.len()
    );
}

fn nav(c: &Controller) {
    check_task_toggle(c);
    let text = c.text();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let total = chars.len() as i32;
    let body = text[..body_start(&text)].chars().count() as i32;
    for (label, step, dir) in [
        ("right (visual)", gtk::MovementStep::VisualPositions, 1),
        ("right (logical)", gtk::MovementStep::LogicalPositions, 1),
        ("left (visual)", gtk::MovementStep::VisualPositions, -1),
    ] {
        let start = if dir > 0 { 0 } else { total };
        c.buffer.place_cursor(&c.buffer.iter_at_offset(start));
        let mut positions = vec![cursor_offset(c)];
        for _ in 0..(total * 3) {
            move_cursor(c, step, dir);
            let now = cursor_offset(c);
            if now == *positions.last().unwrap() {
                break;
            }
            positions.push(now);
        }
        let mut skips = Vec::new();
        for pair in positions.windows(2) {
            let delta = (pair[1] - pair[0]) * dir;
            let (lo, hi) = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            let in_frontmatter = lo < body;
            if delta != 1 && !in_frontmatter {
                skips.push((lo, hi));
            }
        }
        let end = *positions.last().unwrap();
        let reached = if dir > 0 { end == total } else { end <= body };
        println!(
            "== nav {label}: {} steps, {} skipped positions outside the frontmatter, reached the {}: {}",
            positions.len() - 1,
            skips.len(),
            if dir > 0 { "end" } else { "start of the body" },
            reached
        );
        for (lo, hi) in skips.iter().take(6) {
            let a = chars.get(*lo as usize).map_or(text.len(), |c| c.0);
            let b = chars.get(*hi as usize).map_or(text.len(), |c| c.0);
            println!("   skipped {:?}", &text[a..b]);
        }
    }

    // Hidden markers should take no space when concealed by size.
    let concealed = c.concealed();
    if let Some(r) = concealed.iter().find(|r| r.start >= body_start(&text)) {
        let offset = text[..r.start].chars().count() as i32;
        let rect = c.view.iter_location(&c.buffer.iter_at_offset(offset));
        println!(
            "== hidden marker {:?} occupies {}px",
            &text[r.clone()],
            rect.width()
        );
    }

    // Down-arrow through the whole note.
    c.buffer.place_cursor(&c.buffer.iter_at_offset(0));
    let mut lines = 0;
    let mut last = cursor_offset(c);
    for _ in 0..100_000 {
        move_cursor(c, gtk::MovementStep::DisplayLines, 1);
        let now = cursor_offset(c);
        if now == last {
            break;
        }
        if now < last {
            println!("   cursor moved backwards on Down: {last} -> {now}");
        }
        last = now;
        lines += 1;
    }
    println!("== nav down: {lines} display lines, finished at offset {last} of {total}");
    if let Err(e) = c.check_invariants() {
        println!("INVARIANT FAILED: {e}");
    }
}

// --- soak ----------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

const SNIPPETS: &[&str] = &[
    "a",
    "word ",
    "**b**",
    "*i*",
    "\n",
    "\n\n",
    "# ",
    "## H\n",
    "- ",
    "- [ ] ",
    "- [x] ",
    "1. ",
    "[[x]]",
    "[[a|b]]",
    "![[sample.png]]",
    "==h==",
    "~~s~~",
    "`c`",
    "> ",
    "> [!note] t\n",
    "```\n",
    "%%",
    "$m$",
    " ^id",
    "| a | b |\n|---|---|\n",
    "\n---\n",
    "#tag ",
    "é漢😀",
    "https://x.y ",
    "[t](u)",
    "---\na: 1\n---\n",
];

fn soak(c: Rc<Controller>, app: adw::Application, secs: u64, seed: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let rng = Rc::new(RefCell::new(Rng(seed.max(1))));
    let ops = Rc::new(Cell::new(0u64));
    let failures: Rc<RefCell<Vec<String>>> = Rc::default();
    glib::timeout_add_local(Duration::from_millis(1), move || {
        for _ in 0..20 {
            soak_op(&c, &mut rng.borrow_mut());
            ops.set(ops.get() + 1);
            if let Err(e) = c.check_invariants() {
                failures.borrow_mut().push(format!("op {}: {e}", ops.get()));
            }
        }
        if Instant::now() < deadline {
            return glib::ControlFlow::Continue;
        }
        let failures = failures.borrow();
        println!(
            "== soak: {} operations in {secs}s, {} invariant failures, final note {} bytes",
            ops.get(),
            failures.len(),
            c.text().len()
        );
        for f in failures.iter().take(5) {
            println!("   {f}");
        }
        c.with_stats(|s| println!("   {}", summary("restyle", &s.restyle)));
        app.quit();
        glib::ControlFlow::Break
    });
}

fn soak_op(c: &Controller, rng: &mut Rng) {
    let buffer = &c.buffer;
    let total = buffer.char_count();
    let at = |rng: &mut Rng| rng.below(total as u64 + 1) as i32;
    match rng.below(100) {
        0..30 => {
            let snippet = SNIPPETS[rng.below(SNIPPETS.len() as u64) as usize];
            let mut iter = buffer.iter_at_offset(at(rng));
            buffer.begin_user_action();
            buffer.insert(&mut iter, snippet);
            buffer.end_user_action();
        }
        30..48 => {
            let start = at(rng);
            let len = 1 + rng.below(20) as i32;
            let mut a = buffer.iter_at_offset(start);
            let mut b = buffer.iter_at_offset((start + len).min(total));
            buffer.begin_user_action();
            buffer.delete(&mut a, &mut b);
            buffer.end_user_action();
        }
        48..68 => buffer.place_cursor(&buffer.iter_at_offset(at(rng))),
        68..80 => {
            let steps = [
                gtk::MovementStep::VisualPositions,
                gtk::MovementStep::Words,
                gtk::MovementStep::DisplayLines,
                gtk::MovementStep::DisplayLineEnds,
                gtk::MovementStep::Paragraphs,
            ];
            let step = steps[rng.below(steps.len() as u64) as usize];
            let count = if rng.below(2) == 0 { 1 } else { -1 };
            move_cursor(c, step, count);
        }
        80..88 => {
            if rng.below(2) == 0 {
                if buffer.can_undo() {
                    buffer.undo();
                }
            } else if buffer.can_redo() {
                buffer.redo();
            }
        }
        88..94 => {
            let (a, b) = (at(rng), at(rng));
            buffer.select_range(&buffer.iter_at_offset(a), &buffer.iter_at_offset(b));
        }
        _ => {
            let mode = [Mode::Live, Mode::Source, Mode::Reading][rng.below(3) as usize];
            c.set_mode(mode);
            if mode == Mode::Reading && rng.below(2) == 0 {
                c.set_mode(Mode::Live);
            }
        }
    }
}

// --- screenshots ---------------------------------------------------------------

fn screenshot(window: &adw::ApplicationWindow, path: &Path) -> Result<(), String> {
    let content = window.content().ok_or("window has no content")?;
    if !content.is_mapped() {
        return Err("window is not mapped".into());
    }
    let paintable = gtk::WidgetPaintable::new(Some(&content));
    let (w, h) = (content.width() as f64, content.height() as f64);
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w, h);
    let node = snapshot.to_node().ok_or("nothing rendered")?;
    let renderer = window.renderer().ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())
}

// --- sample note ---------------------------------------------------------------

/// Writes a sample note of about `kib` KiB plus an image next to it.
pub fn write_sample(dir: &Path, kib: usize) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = (480usize, 200usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            pixels.extend_from_slice(&[
                (60 + x * 120 / w) as u8,
                (110 + y * 80 / h) as u8,
                200,
                255,
            ]);
        }
    }
    let texture = gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(pixels),
        w * 4,
    );
    texture
        .save_to_png(dir.join("sample.png"))
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut note = String::from(SHOWCASE);
    let mut i = 1;
    while note.len() < kib * 1024 {
        note.push_str(&SECTION.replace("{i}", &i.to_string()));
        i += 1;
    }
    let path = dir.join("sample.md");
    std::fs::write(&path, note)?;
    Ok(path)
}

const SHOWCASE: &str = "---
created: 2026-10-04
tags:
  - igneous
  - spike
up: \"[[Home]]\"
---
# Live Preview prototype

Text with **bold**, *italic*, ==highlight==, ~~strike~~, `inline code`, a [[Wiki Link|wikilink]], a [Markdown link](Other%20note.md), #tag/nested and https://example.com.

- A bullet with a [[link]]
- [ ] An open task
- [x] A finished task
1. A numbered item

> [!tip] Callouts render as panels
> With **formatted** body text.

> A plain quote.

```rust
fn main() {
    println!(\"hello\");
}
```

![[sample.png]]

| Column | Value |
|---|---|
| one | 1 |

Footnote[^1], %%a comment%% and a block ID ^showcase

[^1]: The footnote.

---

";

const SECTION: &str = "## Section {i}

Paragraph {i} with **bold**, *italic*, ==highlight==, `code`, a [[Wiki Link {i}]], a [md link](Note%20{i}.md), #tag/{i} and https://example.com/{i}. A second sentence long enough to wrap across lines when the window is narrow, so layout has some work to do.

- bullet {i} with [[link {i}]]
- [ ] task {i}
- [x] done task {i}

> [!note] Callout {i}
> Body with **bold** text.

```
code block {i}
```

Footnote and %%comment {i}%% ^block-{i}

";
