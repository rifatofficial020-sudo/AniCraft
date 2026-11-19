# ADR 0105: Stage work is submitted inside the frame, and start-up remembers what crashed

* Status: accepted (2026-11-17)
* Relates to: roadmap M3 (GPU stage) and M8 (reliability); issues #264, #309, #335, #255 and the
  "won't open" reports #421, #412, #313, #286, #285, #268, #164; ADR 0006 (cross-platform
  start-up), ADR 0029 and 0032 (the GPU stage on the window's device), ADR 0092 (surviving GPU
  failures)

## Part 1: the stage submits its work only while the app's frame runs

### Context

The most reported failure of 0.2.1 was AniCraft closing as it opened, or when its window was
resized. Where a console showed anything it showed one error (#264):

```text
wgpu error: Validation Error
  In Surface::configure
    Failed to wait for GPU to come idle before reconfiguring the Surface
```

ADR 0092 found why that closed the window (wgpu's default error handler panics) and made
AniCraft survive it. It did not find where the error came from, and blamed load on the GPU.

The error is ours. Before a surface is reconfigured wgpu waits for the device's queue to empty,
then checks that it is empty, and fails if it isn't; its source says of that case "it can only
be non-empty if another thread is submitting at the same time" (`wgpu-core`,
`Device::configure_surface`). The GPU stage draws on the window's device (ADR 0029) from the
render service's threads, and its starting self-test submits from a thread of its own. The
window's thread reconfigures the surface when the window opens and on every resize. Whenever
the two met, the reconfiguration failed.

Measured on Windows 10 with an RTX 5090 on Vulkan, starting with `--screenshot`: the error in
**3 of 20** starts. On 0.2.1 each of those was a start that closed at once, on any GPU. Since
ADR 0092 each is a skipped frame, an autosave and a status line that says the graphics driver
reported a problem, which it had not.

### Decision

1. **A gate decides when work may be submitted on the window's device.** `SubmitGate`
   (`crates/gpu/src/gate.rs`) is open while the window's thread runs the app's frame
   (`eframe::App::ui`): it cannot be reconfiguring a surface then, since that happens on the same
   thread, in eframe, between frames. It is closed the rest of the time. The shell opens it first
   thing in the frame and closes it when the frame ends (`gpu_stage::in_frame`, a guard).
2. **A renderer on the window's device submits through the gate** (`GpuRenderer::shared_gated`):
   the stage's tiles and the starting self-test. A submission that finds the gate closed asks the
   window for a frame and waits for it. Only the hand-over to the queue waits; preparing,
   binning and recording the commands run as before, in parallel.
3. **Windows drawn inside the frame close it for their turn.** Test Movie and floating panels
   are immediate viewports: eframe sets up and draws their surfaces inside our frame. The gate
   is closed around those calls (`gpu_stage::paused`).
4. **Nothing waits for ever.** A submission goes ahead after 250 ms without a frame (a
   minimised window draws none), and closing waits at most 100 ms for submissions under way. Past
   those the old race is possible again and survived as ADR 0092 arranged; the log counts the
   times. The window's own thread never waits: it cannot be reconfiguring while it submits.
5. **A renderer on a device of its own is not gated**: no window reconfigures that device.

### Consequences

* The error is gone where it was measured: 0 of 20 starts, against 3 of 20; a start takes the
  same 2.8 s.
* Work a thread finishes between two frames is submitted when the next frame begins instead of
  at once. The window only showed it in that next frame anyway (a finished tile asks for a
  repaint), so nothing is seen later than before; an idle window draws one more frame per batch
  of tiles.
* ADR 0092's handlers stay: drivers do fail, and the two bounds above let the race back in
  rarely.
* A second thing that submits on the window's device must go through the gate too. There is one
  renderer; `SubmitGate` is public in `anicraft-gpu` for the next.

### Alternatives considered

* **Hand finished command buffers to the window's thread to submit.** The same guarantee and
  the same timing, with more to change: every render call returns before its work is submitted,
  and the error scopes around a submission would move threads.
* **A device of its own for the stage.** No race, and the copy back to the CPU that ADR 0029
  removed returns.
* **DirectX 12 first on Windows.** Tried while looking for the cause: the stage's stress scene
  took 9.1 ms against Vulkan's 5.7 ms at 1920 × 1080 on the same GPU, its shaders took 2.5 s
  longer to build, and the error is not Vulkan's. Dropped.

## Part 2: start-up remembers the ways that crashed

### Context

Start-up tries a list of ways to open the window (`startup::plan`, #207). A driver can crash the
process where nothing catches it (#335: an access violation in Intel's `igvk64.dll`), so each
attempt leaves a marker until the window has drawn; a start that finds the marker puts that way
last. It did so for **that start only**. The start after it found no marker, tried the crashing
way first, and crashed: AniCraft opened every second time.

### Decision

1. **A way that killed a start goes last for 5 starts**, then is tried first again: drivers get
   updated, and writing a way off for ever would leave a fixed machine on a slower one. If it
   kills a start again it goes last for 25 starts, then 100.
2. **Each way has its own count** (`start-died` in the log folder, a line per way): with one
   record, two ways that both crash would take turns.
3. **A way that comes up is forgotten.** One false alarm (the computer switched off as AniCraft
   started, or two copies started at once, the second seeing the first's marker) costs five
   starts on another way, which works, and nothing after.
4. **A way that fails without killing the process** (an error, or a panic that is caught) is
   held back the same way. The next way is tried at once as before; the difference is that the
   starts after it don't open and close a window with it first.
5. **What `--renderer` or `ANICRAFT_RENDERER` asks for is tried first regardless.** It was asked
   for now. The choice in Preferences is not exempt: a choice that crashes must not lock
   AniCraft out.

### Not in this ADR

* A start that hangs inside a driver without crashing still shows nothing until the process is
  ended; the log notes it after 20 s and the next start avoids that way. Restarting itself from
  a watchdog is possible and not done: it needs a machine that hangs to test on.
* The Intel Vulkan crash of #335 still costs its first start on such a machine.
