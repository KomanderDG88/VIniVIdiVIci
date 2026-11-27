//! Host pipelines are made by workers, not by the title thread that first
//! records a draw with them. Translating a shader and the host driver's
//! compile take from milliseconds to seconds each (structurisation of the
//! biggest compute programs ~0.4 s, some driver compiles 3.4 s), and
//! the title's first world frame wants over 1,200 new pipelines: made
//! on its recording threads, that frame took more than 30 s and the
//! title's GPU watchdog ended the run (decision 0007, "Pipelines made off
//! the title's threads").
//!
//! A first use queues the pipeline and waits for it within a budget per
//! frame, so the cheap ones (menus, one-off passes such as a colour-grading
//! bake) are there at once as before; past the budget a draw or dispatch
//! whose pipeline is not ready is dropped, as an untranslated one is.
//!
//! The default budget is half a second per title frame. A pass the title
//! runs only once and whose pipeline takes longer is lost.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Shader;
use super::host::{GraphicsState, HostPipeline, TargetFormats};

/// `VICTORY_DENY_PIPELINES=<16-hex shader hash>,...` (off unless set): the
/// hash of a shader of `shaders` that is listed there. Such a pipeline is
/// never built (no driver compile) and counts as refused, so its draws and
/// dispatches are dropped: a test of whether building a pipeline, rather
/// than running it, is what loses the card (runs 281 and 318: the removal
/// 32 and 81 ms after the 3-second build of `CS_RaytracingResolve` of the
/// two-layer vehicle wheel material, 57a3fad03cac36af and d5b9c94a30d7cf28;
/// gfx41). Unlike `VICTORY_SKIP_PIPELINES`, in release builds too.
fn denied(shaders: &[Arc<Shader>]) -> Option<&str> {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        std::env::var("VICTORY_DENY_PIPELINES")
            .unwrap_or_default()
            .split(',')
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect()
    });
    if list.is_empty() {
        return None;
    }
    shaders.iter().find(|s| list.contains(&s.hash)).map(|s| s.hash.as_str())
}

/// The shaders of `p` (for logs).
pub(crate) fn shaders(p: &Pending) -> &[Arc<Shader>] {
    &p.shaders
}

/// What a host pipeline is made from, and the host pipeline once made.
pub(crate) struct Pending {
    shaders: Vec<Arc<Shader>>,
    pub(crate) graphics: Option<Box<GraphicsState>>,
    queue_state: AtomicU8,
    made: OnceLock<Option<HostPipeline>>,
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    key: String,
    used: AtomicBool,
    /// Whether the made pipeline has gone to a draw or dispatch yet
    /// (`crate::host::first_use`: for a device removal's report).
    handed: AtomicBool,
    /// The formats its state declares for what a draw draws into (zeros
    /// without a graphics state).
    pub(crate) formats: TargetFormats,
    /// The same shaders and state for other formats, bound at a draw
    /// ([`for_formats`]): a chain from the title's pipeline, one link per
    /// set of formats met, never shortened.
    variant: OnceLock<Arc<Pending>>,
    /// A pass over the screen ([`GraphicsState::screen_pass`]) with
    /// [`screen_workers`] on: made by workers of its own, before the
    /// others, and waited for within a budget of its own.
    screen: bool,
    /// When it began to be made (screen passes only: [`given_up`]).
    begun: OnceLock<Instant>,
}

// SAFETY: D3D12 objects are free-threaded; the graphics state (whose input
// elements point into its own names) is only read after creation.
unsafe impl Send for Pending {}
// SAFETY: as above.
unsafe impl Sync for Pending {}

impl Pending {
    pub(crate) fn new(shaders: Vec<Arc<Shader>>, graphics: Option<Box<GraphicsState>>) -> Pending {
        let mut key = blake3::Hasher::new();
        for s in &shaders {
            key.update(s.stage.as_bytes());
            key.update(s.hash.as_bytes());
        }
        key.update(graphics.as_ref().map(|g| g.describe()).unwrap_or_default().as_bytes());
        let screen = screen_workers() && graphics.as_ref().is_some_and(|g| g.screen_pass());
        Pending {
            screen,
            begun: OnceLock::new(),
            formats: graphics.as_ref().map_or([0; 9], |g| g.formats()),
            variant: OnceLock::new(),
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            made: OnceLock::new(),
            key: key.finalize().to_hex()[..32].to_owned(),
            used: AtomicBool::new(false),
            handed: AtomicBool::new(false),
        }
    }

    pub(super) fn key(&self) -> &str {
        &self.key
    }

    fn make(&self) {
        if self.screen {
            let _ = self.begun.set(Instant::now());
        }
        self.made.get_or_init(|| {
            if let Some(hash) = denied(&self.shaders) {
                tracing::warn!(hash, shaders = ?self.shaders, "pipeline not built: its shader is in VICTORY_DENY_PIPELINES");
                return None;
            }
            HostPipeline::new(&self.shaders, self.graphics.as_deref())
        });
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
}

/// Why a pipeline has no host pipeline (diagnostics): its shaders that do
/// not translate, else "refused" or "being made"; `None` if it has one.
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    match p.made.get() {
        Some(Some(_)) => None,
        None => Some("being made".into()),
        Some(None) => {
            let failed: Vec<String> = p
                .shaders
                .iter()
                .filter(|s| matches!(s.translated.get(), Some(None)))
                .map(|s| format!("{} {}", s.stage, s.hash))
                .collect();
            Some(if failed.is_empty() {
                format!("refused {:?}", p.shaders)
            } else {
                failed.join(" + ")
            })
        }
    }
}

/// Signalled whenever a worker has made a pipeline.
static DONE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

/// Waiting allowed per title frame, over all threads.
fn wait_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_PIPELINE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500);
        tracing::info!(millis, "host pipeline wait budget per frame");
        Duration::from_millis(millis)
    })
}

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
    }
    if p.made.get().is_none() {
        start(p, REQUESTED);
        // A compute pipeline in the first 600 frames (the title's one-shot
        // world and lighting set-up passes) or with `compute_waits`: made
        // here, or waited for while a worker makes it, instead of dropping
        // its dispatch after the frame's budget. `make` keeps its panic guard.
        let frame = crate::frame::current();
        // The first 600 frames (loading) make theirs whatever it costs; later
        // frames only while they are under `compute_cap`.
        let early = frame < 600;
        if p.graphics.is_none() && (early || (compute_waits() && compute_room(frame))) {
            let began = std::time::Instant::now();
            p.make();
            if !early {
                compute_spent(frame, began.elapsed());
            }
        } else {
            wait(p, frame);
        }
    }
    let made = p.made.get()?.as_ref();
    // The first time the made pipeline goes to a draw or dispatch: noted
    // for a device removal's report (one load a call after that).
    if let Some(host) = made
        && !p.handed.load(Ordering::Relaxed)
        && !p.handed.swap(true, Ordering::Relaxed)
    {
        crate::host::first_use(&host.name);
    }
    made
}

/// Whether a dispatch waits for its compute pipeline until it is made,
/// whatever the frame's budget ([`wait_budget`]); a draw still waits within
/// the budget and is dropped while its pipeline is not there.
///
/// A dropped draw is drawn again in the next frame. A dropped dispatch may
/// not be: the title runs some compute passes once, when a scene changes
/// (a buffer reset or cleared), and what they leave is read for as long as
/// the scene lasts. In the prison corridor a compute program met for the
/// first time took 4.4 s to make on a 16 GB machine; its one dispatch was
/// dropped after the half second of budget, and the windows of the room
/// before stayed blended over the whole picture for minutes (run 195).
/// A wait of seconds once (the pipeline library has it on the next run)
/// against a picture wrong until the scene ends.
/// Off by default here (unset: within the budget, as before);
/// `VICTORY_PIPELINE_COMPUTE_WAIT=1` turns it on.
fn compute_waits() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_PIPELINE_COMPUTE_WAIT").is_some_and(|v| v == "1");
        if on {
            tracing::info!("dispatches wait for their compute pipelines until made (VICTORY_PIPELINE_COMPUTE_WAIT=1)");
        }
        on
    })
}

/// The most a frame waits for compute pipelines made at their dispatch
/// ([`compute_waits`]), over all threads, before its later dispatches go
/// back to the frame's budget ([`wait`]). With a new cache the first
/// cutscene met 457 compute programs in one frame at about a second each:
/// the frame did not end for over 30 s and the title's own hang watchdog
/// stored to address 0. Capped, the frame ends and the rest are made by
/// the workers and waited for within the budget, as before
/// [`compute_waits`]. The check comes before a make, so a frame can run
/// over by the makes already started on other threads (seconds each):
/// `VICTORY_PIPELINE_COMPUTE_WAIT_MS`, 8,000 by default, leaves room for
/// that under the watchdog's 30 s.
fn compute_cap() -> std::time::Duration {
    static MS: OnceLock<u64> = OnceLock::new();
    std::time::Duration::from_millis(*MS.get_or_init(|| {
        std::env::var("VICTORY_PIPELINE_COMPUTE_WAIT_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(8_000)
    }))
}

/// The frame whose compute waits are counted, and the microseconds counted.
static COMPUTE_WAITED: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));

/// Whether frame `frame` may still wait for a compute pipeline ([`compute_cap`]).
fn compute_room(frame: u64) -> bool {
    let mut w = COMPUTE_WAITED.lock().unwrap_or_else(|e| e.into_inner());
    if w.0 != frame {
        *w = (frame, 0);
    }
    let room = u128::from(w.1) < compute_cap().as_micros();
    if !room {
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                frame,
                cap_ms = u64::try_from(compute_cap().as_millis()).unwrap_or(u64::MAX),
                "a frame waited its cap for compute pipelines; the rest wait within its budget (said once)"
            );
        }
    }
    room
}

/// Counts a compute wait to `frame`, if that is still the frame counted.
fn compute_spent(frame: u64, took: std::time::Duration) {
    let mut w = COMPUTE_WAITED.lock().unwrap_or_else(|e| e.into_inner());
    if w.0 == frame {
        w.1 = w.1.saturating_add(u64::try_from(took.as_micros()).unwrap_or(u64::MAX));
    }
}

/// Pipelines made again for the formats bound at a draw, since the start.
static VARIANTS: AtomicU64 = AtomicU64::new(0);

/// Whether a draw into views of other formats than its pipeline declares
/// gets a host pipeline for those formats ([`for_formats`]). On by
/// default; `VICTORY_TARGET_VARIANTS=0` sends such a draw to the host with
/// the title's pipeline, as before.
fn variants_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TARGET_VARIANTS").is_none_or(|v| v != "0"))
}

/// The host pipeline for a draw with `p` into views of the formats
/// `bound`, which `p` does not declare (`host::formats_differ`): that of
/// the same shaders and state for those formats. It is a pipeline like the
/// title's own: queued at its first draw, made by a worker, waited for
/// within the frame's budget, and the draw is dropped while it is not
/// there ([`ready`]).
#[cold]
pub(crate) fn for_formats<'a>(p: &'a Arc<Pending>, bound: &TargetFormats) -> Option<&'a HostPipeline> {
    match p.graphics.as_deref() {
        Some(g) if variants_on() => ready(variant(p, g, &super::host::formats_for(&p.formats, bound))),
        _ => ready(p),
    }
}

/// The pipeline of `p`'s shaders and its state `g` for the formats
/// `bound`: found in the chain that starts at `p`, or added to it (logged
/// then: once per pipeline and set of formats).
fn variant<'a>(p: &'a Arc<Pending>, g: &GraphicsState, bound: &TargetFormats) -> &'a Arc<Pending> {
    let mut at = p;
    // (`bound` is what `host::formats_for` gives, and a state made for it
    // declares exactly it, so the walk ends at the link made for it; the
    // count is a bound all the same: past it the title's own pipeline.)
    for _ in 0..64 {
        let next = at.variant.get_or_init(|| {
            let so_far = VARIANTS.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(
                pipeline = ?p.shaders,
                declared = ?p.formats,
                bound = ?bound,
                so_far,
                "a pipeline is made again for the render target formats bound at its draw (eight render targets, then depth; VICTORY_TARGET_VARIANTS=0: the draw goes out with the title's pipeline)"
            );
            Arc::new(Pending::new(p.shaders.clone(), Some(Box::new(g.with_formats(bound)))))
        });
        // (A state made for `bound` declares exactly `bound`.)
        if next.formats == *bound {
            return next;
        }
        at = next;
    }
    p
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// Whether `p` has been made (with or without a host pipeline).
#[cfg(feature = "diagnostics")]
pub(crate) fn made(p: &Pending) -> bool {
    p.made.get().is_some()
}

/// At the title's creation of a pipeline that must be made before its
/// first use, all at once with others (the ray-tracing structure
/// builder's, with `VICTORY_BVH_BUILDER=1`): queued at once, ahead of the
/// pipelines draws have asked for. Made only on demand, the builder's were
/// never queued (its dispatches are dropped until it runs, and its first
/// one made all 53 in place: a 30 s frame, after which the title ended
/// itself). Without workers, nothing (made in place as before).
pub(crate) fn ahead(p: &Arc<Pending>) {
    if p.shaders.is_empty()
        || p.queue_state.load(Ordering::Relaxed) >= REQUESTED
        || !workers()
    {
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push_ahead(p) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        AVAILABLE.notify_one();
    }
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it).
pub(crate) fn early(p: &Arc<Pending>) {
    if !p.shaders.is_empty() && super::library::wanted_early(&p.key) {
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(p, EARLY);
    }
}

/// Whether the frame's budget is one for all threads together: a thread
/// that waits takes what is left of it and gives back what it did not
/// use. Counted only when a wait had ended, the budget was one per thread:
/// each of the title's five recording threads began its first wait of a
/// frame with nothing counted yet and waited the whole half second, 2.5 s
/// of waiting in every frame in which pipelines were being made (runs
/// 34-49: "2504 ms waiting for pipelines" in each such frame).
/// On by default; `VICTORY_PIPELINE_WAIT_SHARED=0` counts as before.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0"))
}

/// Whether `p` is a screen pass the host's driver has been at for longer
/// than [`SCREEN_BUDGET`]: no frame waits for it any more (a driver
/// compile that never ends would otherwise cost every frame the whole
/// budget). Never for other pipelines.
fn given_up(p: &Pending) -> bool {
    p.screen && p.begun.get().is_some_and(|at| at.elapsed() > SCREEN_BUDGET)
}

/// Waits for `p` while frame `frame` has budget left.
fn wait(p: &Pending, frame: u64) {
    let budget = wait_budget();
    if budget.is_zero() || given_up(p) {
        return;
    }
    // Screen passes: a budget of their own, longer ([`screen_workers`]).
    let budget = if p.screen { SCREEN_BUDGET } else { budget };
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now); the second is the screen passes'.
    static SPENTS: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let frame_spent = &SPENTS[usize::from(p.screen)];
    let shared = shared_budget();
    let left = {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        return;
    }
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !given_up(p) {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        // A screen pass's wait is woken now and then to see whether it has
        // been given up on.
        let rest = if p.screen { rest.min(Duration::from_millis(250)) } else { rest };
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        // A frame that began meanwhile has its own budget.
        if spent.0 == frame {
            if shared {
                spent.1 = spent.1.saturating_sub(left - waited);
            } else {
                spent.1 += waited;
            }
        }
    }
    crate::framelog::add(crate::framelog::T::PipelineWaitUs, started.elapsed().as_micros() as u64);
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
}

const NOT_QUEUED: u8 = 0;
const EARLY: u8 = 1;
const REQUESTED: u8 = 2;
const MAKING: u8 = 3;

struct Queue {
    early: VecDeque<Arc<Pending>>,
    requested: VecDeque<Arc<Pending>>,
    /// Pipelines queued at creation to be made before any requested one
    /// ([`ahead`]; empty without `VICTORY_BVH_BUILDER=1`).
    ahead: VecDeque<Arc<Pending>>,
    /// Requested screen passes ([`screen_workers`]): before the others,
    /// and the only work of the screen workers. Always empty with the
    /// switch off.
    screen: VecDeque<Arc<Pending>>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            early: VecDeque::new(),
            requested: VecDeque::new(),
            ahead: VecDeque::new(),
            screen: VecDeque::new(),
        }
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED && p.screen {
            self.screen.push_back(p.clone());
        } else if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else {
            self.early.push_back(p.clone());
        }
        Some(prev == NOT_QUEUED)
    }

    /// Queues `p` as requested, ahead of the requested pipelines
    /// ([`ahead`]); as [`Queue::push`].
    fn push_ahead(&mut self, p: &Arc<Pending>) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= REQUESTED {
            return None;
        }
        p.queue_state.store(REQUESTED, Ordering::Relaxed);
        self.ahead.push_back(p.clone());
        Some(prev == NOT_QUEUED)
    }

    fn pop(&mut self) -> Option<Arc<Pending>> {
        let p = self
            .screen
            .pop_front()
            .or_else(|| self.ahead.pop_front())
            .or_else(|| self.requested.pop_front())
            .or_else(|| {
                // An early entry can have been promoted and claimed already.
                std::iter::from_fn(|| self.early.pop_front())
                    .find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
            })?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }

    /// The next screen pass to make, if one is queued (the screen
    /// workers' only work).
    fn pop_screen(&mut self) -> Option<Arc<Pending>> {
        let p = self.screen.pop_front()?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static AVAILABLE: Condvar = Condvar::new();
/// Signalled when a screen pass is queued: the screen workers wait on
/// this, not on [`AVAILABLE`], so that they never take a wake-up meant for
/// the other workers (a pipeline left queued with them all asleep).
static SCREEN_AVAILABLE: Condvar = Condvar::new();

/// Start or promote work for a draw without waiting behind the entire early
/// list. Queue changes and worker claims happen under the same lock.
fn start(p: &Arc<Pending>, priority: u8) {
    if p.queue_state.load(Ordering::Relaxed) >= priority {
        return;
    }
    if !workers() {
        if p.queue_state.swap(MAKING, Ordering::Relaxed) == NOT_QUEUED {
            p.make();
        }
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push(p, priority) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        if p.screen && priority == REQUESTED {
            SCREEN_AVAILABLE.notify_one();
        }
        AVAILABLE.notify_one();
    }
}

fn next() -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop() {
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// The next screen pass to make (the screen workers').
fn next_screen() -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop_screen() {
            return p;
        }
        queue = SCREEN_AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// Whether passes over the screen (no vertex input, no depth or stencil
/// test: [`GraphicsState::screen_pass`]) have workers and a wait budget of
/// their own: `VICTORY_SCREEN_WORKERS=1` (off by default; unset, pipelines
/// are made and waited for exactly as before).
///
/// Why: the title fills its lighting tables (`PS_IntegrateEnvBRDF*`,
/// `PS_AreaLightIlluminance`) with draws it makes ONCE, at the world's
/// first frame. Behind a world's worth of pipelines to make (hundreds) they
/// waited past the frame's budget and were dropped, and never drawn again:
/// no diffuse light, a grey, sepia picture (white bloom indoors). With it,
/// two workers of their own make such pipelines first (the others take
/// them first too), and a draw with one waits up to [`SCREEN_BUDGET`] in a
/// frame instead of being dropped after the usual budget.
///
/// Risk: a hitch of up to 12 s in a frame (on first entering the world,
/// with an empty pipeline cache) while those pipelines are made; and two
/// more threads compiling at normal priority beside the title's.
/// It changes only which pipelines are made first and how long a frame
/// waits for them, never what a draw does.
///
/// Adapted from ZigZagBoi1/ScarlettPort runtime/victory (fx-cand4, commit
/// 549daebc; Belobug 18e57db), where it is on by default.
fn screen_workers() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SCREEN_WORKERS").is_some_and(|v| v == "1"))
}

/// Waiting allowed per title frame for screen passes ([`screen_workers`]),
/// below the title's 30 s GPU watchdog with the usual budget beside it
/// (also how long the driver may be at one before no frame waits for it:
/// [`given_up`]).
const SCREEN_BUDGET: Duration = Duration::from_secs(12);

/// Starts the workers (half the logical processors, 2 to 8) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        // `VICTORY_PIPELINE_WORKERS=<n>` overrides the count;
        // `VICTORY_PIPELINE_NORMAL=1` runs them at normal priority, level
        // with the title's threads (a title holding every core starves
        // below-normal workers: about one pipeline per 2 s was seen).
        let workers = std::env::var("VICTORY_PIPELINE_WORKERS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|n| n.clamp(1, 64))
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 8)));
        let normal = std::env::var_os("VICTORY_PIPELINE_NORMAL").is_some_and(|v| v == "1");
        tracing::info!(workers, normal, "host pipeline workers");
        // Two of their own for screen passes ([`screen_workers`]), at
        // normal priority: their pipelines are few, and a frame waits for
        // them.
        let screen = if screen_workers() { 2 } else { 0 };
        if screen > 0 {
            tracing::info!(screen, "host pipeline workers for screen passes (VICTORY_SCREEN_WORKERS=1)");
        }
        for i in 0..screen {
            let spawned = std::thread::Builder::new()
                .name(format!("victory-pipelines-screen-{i}"))
                .spawn(|| {
                    loop {
                        next_screen().make();
                        if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                            drained();
                        }
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "no screen pass pipeline worker");
            }
        }
        for i in 0..workers {
            let spawned = std::thread::Builder::new()
                .name(format!("victory-pipelines-{i}"))
                .spawn(move || {
                    // Below the title's own threads (loading, recording),
                    // unless asked otherwise.
                    // SAFETY: a plain call on our own thread.
                    if !normal {
                        unsafe {
                            use windows::Win32::System::Threading::*;
                            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                        }
                    }
                    loop {
                        next().make();
                        if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                            drained();
                        } else {
                            super::library::save(false);
                        }
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "no pipeline worker");
            } else {
                running = true;
            }
        }
        running
    })
}

/// The most pipelines waiting at once since the workers last ran out.
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The workers ran out of work: logged after a batch of 10 or more, and
/// the cache saved (at most every 20 s).
fn drained() {
    use super::library::{EARLY, LOADED, MADE};
    if PEAK.swap(0, Ordering::Relaxed) >= 10 {
        tracing::info!(
            made = MADE.load(Ordering::Relaxed),
            loaded = LOADED.load(Ordering::Relaxed),
            started_early = EARLY.load(Ordering::Relaxed),
            "host pipelines all made"
        );
    }
    super::library::save(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first use queues the pipeline and, within the budget, gets the
    /// worker's answer (here: no host pipeline for no shaders).
    #[test]
    fn first_use_is_made_by_a_worker() {
        let p = Arc::new(Pending::new(Vec::new(), None));
        assert!(ready(&p).is_none());
        assert_ne!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        let started = Instant::now();
        while p.made.get().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)));
    }

    /// A draw into other formats than its pipeline declares finds the
    /// pipeline for them, which exists once per set of formats and is
    /// queued by its first draw like any other.
    #[test]
    fn a_pipeline_for_bound_formats_is_made_once() {
        // The title's pipeline declares one render target, format 24.
        let p = Arc::new(Pending::new(Vec::new(), Some(Box::new(GraphicsState::declaring(24)))));
        assert_eq!(p.formats, [24, 0, 0, 0, 0, 0, 0, 0, 0]);
        let g = p.graphics.as_deref().unwrap();
        let (a, b) = ([26, 0, 0, 0, 0, 0, 0, 0, 0], [28, 0, 0, 0, 0, 0, 0, 0, 0]);
        let first = variant(&p, g, &a);
        assert_eq!(first.formats, a);
        assert_ne!(first.key, p.key);
        assert!(Arc::ptr_eq(first, variant(&p, g, &a)));
        let second = variant(&p, g, &b);
        assert_eq!(second.formats, b);
        assert!(!Arc::ptr_eq(first, second));
        assert_ne!(second.key, first.key);
        assert!(Arc::ptr_eq(first, variant(&p, g, &a)));
        assert!(Arc::ptr_eq(second, variant(&p, g, &b)));
        assert_eq!(first.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        assert_eq!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
    }

    #[test]
    fn pending_pipeline_is_recorded_for_the_next_launch() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "pending-pipeline-test".to_owned();
        // Simulate a worker that has taken the job but cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let p = Arc::new(p);

        assert!(!super::super::library::wanted_early(&p.key));
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none());
        assert!(super::super::library::wanted_early(&p.key));
    }

    #[test]
    fn draws_promote_early_pipelines_without_claiming_them_twice() {
        let earlier = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&earlier, EARLY), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert_eq!(queue.push(&promoted, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &promoted));
        assert_eq!(queue.push(&promoted, REQUESTED), None);
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &earlier));
        assert!(queue.pop().is_none());
    }

    /// A pipeline queued ahead (the structure builder's) is made before
    /// the requested ones, in the order queued, and once.
    #[test]
    fn pipelines_queued_ahead_come_before_requested_ones() {
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let first = Arc::new(Pending::new(Vec::new(), None));
        let second = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&first, EARLY), Some(true));
        assert_eq!(queue.push_ahead(&first), Some(false));
        assert_eq!(queue.push_ahead(&second), Some(true));
        assert_eq!(queue.push_ahead(&second), None);
        assert_eq!(queue.push(&second, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &first));
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &second));
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &requested));
        assert!(queue.pop().is_none());
    }

    /// A requested screen pass is made before the other requested
    /// pipelines, by any worker, and is the screen workers' only work; one
    /// queued early goes to them when a draw requests it.
    #[test]
    fn screen_passes_come_first_and_alone_to_their_workers() {
        let screen = || {
            let mut p = Pending::new(Vec::new(), None);
            p.screen = true;
            Arc::new(p)
        };
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let (first, promoted) = (screen(), screen());
        let mut queue = Queue::new();

        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&first, REQUESTED), Some(true));
        // The other workers: the screen pass before what was asked first.
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &first));
        // The screen workers: no other work, nor an unrequested screen pass.
        assert!(queue.pop_screen().is_none());
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert!(Arc::ptr_eq(&queue.pop_screen().unwrap(), &promoted));
        assert_eq!(promoted.queue_state.load(Ordering::Relaxed), MAKING);
        assert!(queue.pop_screen().is_none());
        // Its early entry is not made again.
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &requested));
        assert!(queue.pop().is_none());
    }

    /// Only a screen pass is given up on, and only once the driver has been
    /// at it for longer than its budget.
    #[test]
    fn only_slow_screen_passes_are_given_up_on() {
        let old = Instant::now().checked_sub(SCREEN_BUDGET + Duration::from_secs(1));
        let Some(old) = old else { return };
        let mut p = Pending::new(Vec::new(), None);
        assert!(!given_up(&p));
        p.begun.set(old).unwrap();
        assert!(!given_up(&p));
        p.screen = true;
        assert!(given_up(&p));
        let mut q = Pending::new(Vec::new(), None);
        q.screen = true;
        assert!(!given_up(&q));
        q.begun.set(Instant::now()).unwrap();
        assert!(!given_up(&q));
    }
}
