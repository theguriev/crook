//! One plugin, running.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crook_plugin_api::{ABI_VERSION, Answer, Manifest, Node, Registered, Render, Request};
use wasmi::{Caller, Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

use crate::host::Registry;
use crate::{exports, imports, unpack};

/// How many instructions one call may take.
///
/// A budget rather than a timer, because a timer is a wall clock and a wall
/// clock makes a plugin behave differently on a loaded machine — which is a
/// plugin that fails in the field and not in its author's tests. Fuel is
/// deterministic: the same input costs the same anywhere.
///
/// The numbers are what they are because of *what the call is*. A render runs
/// once per frame per slot, so it gets what a frame can afford; a build runs
/// once, so it can afford to be slow; an action is a person waiting for a
/// click, so it gets more than a frame and less than a hang.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fuel {
    /// What `crook_build` may spend.
    pub build: u64,
    /// What one `crook_render` may spend.
    pub render: u64,
    /// What one `crook_run` may spend.
    pub run: u64,
    /// What one `crook_deliver` or `crook_tick` may spend.
    pub event: u64,
}

impl Default for Fuel {
    fn default() -> Self {
        Self {
            // Generous: this happens once, while a window is opening, and a
            // plugin that reads a file it shipped with is doing it here.
            build: 50_000_000,
            // A frame at 60Hz is 16ms and this is one contribution in it. A
            // million is a little under a millisecond: `portable-dispatch`
            // wasmi in a release build was measured here at about 1.2 billion
            // of these a second, and a chip that draws a mark, a percentage
            // and a panel of totals under it costs a few hundred thousand.
            //
            // The number these were first set from — a hundred million a
            // second — came from a guess and was twelve times pessimistic.
            // Measuring it changed nothing about the budgets that matter and
            // everything about what they mean: this one is a millisecond, not
            // ten.
            render: 1_000_000,
            // A person clicked and is waiting. Slower than a frame is fine;
            // slower than a second is not.
            run: 100_000_000,
            // Nobody is waiting on this one — it is an answer landing or a
            // timer going off, both off the frame path — but it *is* where a
            // plugin does its real work, and the work is not always small: a
            // week of totals counted out of three hundred megabytes of
            // transcripts arrives as a few hundred rows, and taking them in
            // costs a few million. Forty is about thirty milliseconds, which
            // is two frames on the one occasion a panel is opened, and far
            // more than anything that happens every minute.
            //
            // It was ten million, which a real week just exceeded — and a
            // budget a real answer cannot fit in is a feature that works until
            // somebody has been using the machine for a week.
            event: 40_000_000,
        }
    }
}

/// How much memory a plugin may have, in 64KiB wasm pages.
///
/// Sixteen megabytes. Enough for a plugin that holds a page of JSON it fetched
/// and not enough to be how a machine runs out of memory. A plugin that asks
/// for more at instantiation does not instantiate; one that grows into it and
/// asks for more gets `-1` from `memory.grow`, which is a failure its own
/// allocator has to deal with rather than the host's problem.
const MEMORY_PAGES: u32 = 256;

/// The longest string a guest may hand back in one call.
///
/// Not a guess at what is reasonable: it is a bound on what one bad `i64` can
/// make the host allocate. A guest that returns a length of four billion
/// should cost a refused call rather than four gigabytes.
const MAX_ANSWER: u32 = 1 << 20;

/// Why a plugin did not do what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Built against a version of the vocabulary this build does not know.
    Abi {
        /// What the plugin says it was built against.
        theirs: u32,
        /// What this build speaks.
        ours: u32,
    },
    /// Not valid WebAssembly, or valid and missing something it must export.
    Shape(String),
    /// It ran out of fuel, or trapped, or the call failed.
    Ran(String),
    /// It handed back a length or an offset that is not inside its memory, or
    /// bytes that are not what they were supposed to be.
    Answer(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Abi { theirs, ours } => write!(
                formatter,
                "built for plugin API {theirs}, and this is Crook {ours}"
            ),
            Self::Shape(why) => write!(formatter, "not a plugin this build can run: {why}"),
            Self::Ran(why) => write!(formatter, "stopped while running: {why}"),
            Self::Answer(why) => write!(formatter, "answered with something unreadable: {why}"),
        }
    }
}

impl std::error::Error for Problem {}

/// One plugin's module, instantiated and ready to be asked things.
pub struct Sandbox {
    store: Store<Registry>,
    instance: Instance,
    memory: Memory,
    fuel: Fuel,
    registry: Registry,
}

impl Sandbox {
    /// Instantiates `wasm`, checks the ABI, and reads the manifest.
    ///
    /// Everything that can refuse a plugin happens here and in this order, so
    /// that the first thing wrong is the thing reported: a module that is not
    /// wasm is not asked for its ABI version, and one built for another
    /// version is not asked for its manifest.
    pub fn open(wasm: &[u8], fuel: Fuel) -> Result<(Self, Manifest), Problem> {
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|why| Problem::Shape(why.to_string()))?;

        let registry = Registry::new();
        let mut store = Store::new(&engine, registry.clone());
        // Set before anything runs, because instantiation runs the module's
        // start function and its data segments, and a module whose start
        // function loops is a module that has to stop too.
        store
            .set_fuel(fuel.build)
            .map_err(|why| Problem::Ran(why.to_string()))?;

        let mut linker = Linker::new(&engine);
        install(&mut linker).map_err(|why| Problem::Shape(why.to_string()))?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|why| Problem::Shape(why.to_string()))?;

        let memory = instance
            .get_memory(&store, exports::MEMORY)
            .ok_or_else(|| Problem::Shape(format!("it exports no {:?}", exports::MEMORY)))?;
        if memory.size(&store) > u64::from(MEMORY_PAGES) {
            return Err(Problem::Shape(format!(
                "it wants {} pages of memory and the limit is {MEMORY_PAGES}",
                memory.size(&store)
            )));
        }

        let mut sandbox = Self {
            store,
            instance,
            memory,
            fuel,
            registry,
        };

        // The ABI first, because everything below it is decoded against a
        // shape this version defines.
        let theirs = sandbox
            .call::<(), i32>(exports::ABI_VERSION, (), sandbox.fuel.build)?
            .max(0) as u32;
        if theirs != ABI_VERSION {
            return Err(Problem::Abi {
                theirs,
                ours: ABI_VERSION,
            });
        }

        let packed = sandbox.call::<(), i64>(exports::MANIFEST, (), sandbox.fuel.build)?;
        let bytes = sandbox.read(packed)?;
        let manifest: Manifest =
            crook_plugin_api::from_bytes(&bytes).map_err(|why| Problem::Answer(why.to_string()))?;

        Ok((sandbox, manifest))
    }

    /// Runs the plugin's `build`, and hands back everything it registered.
    ///
    /// A build that fails registers *nothing*: whatever it managed to record
    /// before it gave up is thrown away, so a half-built plugin never leaves
    /// half a contribution on a slot.
    pub fn build(&mut self) -> Result<Registered, Problem> {
        self.registry.clear();
        let outcome = self.call::<(), i32>(exports::BUILD, (), self.fuel.build);

        match outcome {
            Ok(0) => Ok(self.registry.taken()),
            Ok(other) => {
                self.registry.clear();
                Err(Problem::Ran(format!("its build answered {other}")))
            }
            Err(problem) => {
                self.registry.clear();
                Err(problem)
            }
        }
    }

    /// Asks what it wants drawn in one slot, for one subject.
    ///
    /// The subject is the host's answer to a slot that is drawn more than
    /// once: a mark in the tab panel is asked for once per row, and a plugin
    /// told only the slot name would have to give every row the same mark.
    /// What is *in* a subject is decided above this file — see the
    /// application's `plugins::wasm`, which is where a grant is compared
    /// against what a plugin asked to be allowed to know.
    pub fn render(&mut self, render: &Render) -> Result<Node, Problem> {
        let bytes =
            crook_plugin_api::to_bytes(render).map_err(|why| Problem::Answer(why.to_string()))?;
        let (pointer, length) = self.write(&bytes, self.fuel.render)?;
        let packed =
            self.call::<(i32, i32), i64>(exports::RENDER, (pointer, length), self.fuel.render)?;
        let bytes = self.read(packed)?;
        crook_plugin_api::from_bytes(&bytes).map_err(|why| Problem::Answer(why.to_string()))
    }

    /// Runs one of its actions, by the name it registered.
    pub fn run(&mut self, action: &str) -> Result<(), Problem> {
        let (pointer, length) = self.write(action.as_bytes(), self.fuel.run)?;
        match self.call::<(i32, i32), i32>(exports::RUN, (pointer, length), self.fuel.run)? {
            0 => Ok(()),
            other => Err(Problem::Ran(format!("the action answered {other}"))),
        }
    }

    /// Tells the guest how far this machine's own time is from UTC, in
    /// minutes east of it.
    ///
    /// Set by whoever owns a clock, which is not this crate. Re-set rather
    /// than read once: an offset changes when the clocks go back and when a
    /// laptop is opened in another country, and a chart of days drawn against
    /// the wrong one is wrong in a way nobody would think to check.
    pub fn set_timezone(&mut self, minutes: i32) {
        self.registry.set_timezone(minutes);
    }

    /// How much of the last call's budget was left when it returned.
    ///
    /// For finding out what a call actually costs rather than guessing: a
    /// budget nobody has measured is a budget that is either wasted or about
    /// to be exceeded on somebody else's machine.
    pub fn fuel_left(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }

    /// Everything the guest asked the host to do since it was last asked,
    /// with the ticket each answer must carry.
    ///
    /// Drained rather than read: a request handed over twice would be a
    /// network call made twice, and the second one would be the host's fault.
    pub fn asked(&mut self) -> Vec<(u32, Request)> {
        self.registry.asked()
    }

    /// How long the guest asked to be left alone for, if it asked at all.
    ///
    /// Also drained. A timer that stayed set would be a plugin that keeps
    /// being ticked long after it stopped asking to be.
    pub fn timer(&mut self) -> Option<Duration> {
        self.registry.timer()
    }

    /// Hands the guest the answer to something it asked for.
    ///
    /// Whether it *wanted* an answer is [`Sandbox::takes_answers`]: a module
    /// with no `crook_deliver` gets nothing delivered rather than an error,
    /// because a plugin that asks for nothing is a perfectly good plugin.
    pub fn deliver(&mut self, ticket: u32, answer: &Answer) -> Result<(), Problem> {
        let bytes =
            crook_plugin_api::to_bytes(answer).map_err(|why| Problem::Answer(why.to_string()))?;
        let (pointer, length) = self.write(&bytes, self.fuel.event)?;
        let ticket = i32::try_from(ticket)
            .map_err(|_| Problem::Answer("the ticket is not a number a guest can hold".into()))?;

        match self.call::<(i32, i32, i32), i32>(
            exports::DELIVER,
            (ticket, pointer, length),
            self.fuel.event,
        )? {
            0 => Ok(()),
            other => Err(Problem::Ran(format!(
                "it answered {other} to something it asked for"
            ))),
        }
    }

    /// Tells the guest the wait it asked for has passed.
    pub fn tick(&mut self) -> Result<(), Problem> {
        match self.call::<(), i32>(exports::TICK, (), self.fuel.event)? {
            0 => Ok(()),
            other => Err(Problem::Ran(format!("its tick answered {other}"))),
        }
    }

    /// Whether this module has somewhere to put an answer.
    pub fn takes_answers(&self) -> bool {
        self.instance
            .get_func(&self.store, exports::DELIVER)
            .is_some()
    }

    /// Whether this module has somewhere to put a tick.
    pub fn takes_ticks(&self) -> bool {
        self.instance.get_func(&self.store, exports::TICK).is_some()
    }

    /// Calls one export with its own budget.
    ///
    /// The fuel is set per call rather than once, and that is the difference
    /// between a plugin that may run for a while and a plugin that may run for
    /// a while *once*. A budget that was spent over a session would make a
    /// plugin stop working after an hour of doing nothing wrong.
    fn call<Args, Ret>(&mut self, name: &str, args: Args, fuel: u64) -> Result<Ret, Problem>
    where
        Args: wasmi::WasmParams,
        Ret: wasmi::WasmResults,
    {
        let function: TypedFunc<Args, Ret> = self
            .instance
            .get_typed_func(&self.store, name)
            .map_err(|why| Problem::Shape(format!("{name}: {why}")))?;
        self.store
            .set_fuel(fuel)
            .map_err(|why| Problem::Ran(why.to_string()))?;
        function
            .call(&mut self.store, args)
            .map_err(|why| Problem::Ran(format!("{name}: {why}")))
    }

    /// Copies `bytes` into the guest's memory, through the guest's allocator.
    ///
    /// Through *its* allocator, because the host has no idea which bytes of a
    /// guest's memory are free — and a host that wrote wherever it liked would
    /// be corrupting the plugin it is trying to talk to.
    ///
    /// The allocation is charged to the budget of the call it is for: putting
    /// a slot's name in front of a render is part of that render, and a
    /// megabyte of answer that has to be allocated before `crook_deliver` sees
    /// it is part of the delivery.
    fn write(&mut self, bytes: &[u8], fuel: u64) -> Result<(i32, i32), Problem> {
        let length = i32::try_from(bytes.len())
            .map_err(|_| Problem::Answer("it is longer than the guest can address".into()))?;
        let pointer = self.call::<i32, i32>(exports::ALLOC, length, fuel)?;
        let start = usize::try_from(pointer).map_err(|_| {
            Problem::Answer("its allocator answered with a negative address".into())
        })?;

        self.memory
            .write(&mut self.store, start, bytes)
            .map_err(|why| Problem::Answer(why.to_string()))?;
        Ok((pointer, length))
    }

    /// Reads back the slice a guest packed into an `i64`.
    ///
    /// Every bound is checked here rather than trusted, because the number
    /// came from the plugin: a length past the end of its memory, a pointer
    /// past the end, and a length no answer could honestly be are each a
    /// refused answer instead of a read of somebody else's bytes.
    fn read(&mut self, packed: i64) -> Result<Vec<u8>, Problem> {
        let (pointer, length) = unpack(packed);
        if length > MAX_ANSWER {
            return Err(Problem::Answer(format!(
                "it answered with {length} bytes and the limit is {MAX_ANSWER}"
            )));
        }

        let data = self.memory.data(&self.store);
        let start = pointer as usize;
        let end = start
            .checked_add(length as usize)
            .ok_or_else(|| Problem::Answer("its answer wraps past the end of memory".into()))?;
        let bytes = data
            .get(start..end)
            .ok_or_else(|| Problem::Answer("its answer is not inside its own memory".into()))?;

        Ok(bytes.to_vec())
    }
}

/// Puts the host functions in the linker.
fn install(linker: &mut Linker<Registry>) -> Result<(), wasmi::Error> {
    linker.func_wrap(
        imports::MODULE,
        imports::CONTRIBUTE,
        |mut caller: Caller<'_, Registry>,
         slot: i32,
         slot_len: i32,
         entry: i32,
         entry_len: i32,
         order: i32| {
            let Some(slot) = string_at(&mut caller, slot, slot_len) else {
                return;
            };
            let Some(entry) = string_at(&mut caller, entry, entry_len) else {
                return;
            };
            caller.data().contribute(slot, entry, order);
        },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::REGISTER_ACTION,
        |mut caller: Caller<'_, Registry>, name: i32, name_len: i32, title: i32, title_len: i32| {
            let Some(name) = string_at(&mut caller, name, name_len) else {
                return;
            };
            // A zero-length title is an action that is reachable and not
            // offered — the arrow keys a palette binds to itself.
            let title = (title_len > 0)
                .then(|| string_at(&mut caller, title, title_len))
                .flatten();
            caller.data().register_action(name, title);
        },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::REQUEST,
        |mut caller: Caller<'_, Registry>, pointer: i32, length: i32| -> i32 {
            let Some(bytes) = bytes_at(&mut caller, pointer, length) else {
                return 0;
            };
            let Ok(request) = crook_plugin_api::from_bytes::<Request>(&bytes) else {
                // Zero rather than a trap, for the reason `string_at` answers
                // `None`: a plugin that encoded its own request wrongly has a
                // bug, and its bug must not be the window's.
                return 0;
            };
            caller.data().ask(request) as i32
        },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::TIMER,
        |caller: Caller<'_, Registry>, millis: i32| -> i32 {
            let Ok(millis) = u64::try_from(millis) else {
                return 0;
            };
            caller
                .data()
                .wants_ticking_in(Duration::from_millis(millis));
            0
        },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::NOW,
        |_: Caller<'_, Registry>| -> i64 {
            // A clock before the epoch is a machine whose clock is wrong, and
            // zero is a more useful thing to hand a plugin than a panic.
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(0)
        },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::TIMEZONE,
        |caller: Caller<'_, Registry>| -> i32 { caller.data().timezone() },
    )?;

    linker.func_wrap(
        imports::MODULE,
        imports::LOG,
        |mut caller: Caller<'_, Registry>, level: i32, text: i32, length: i32| {
            let Some(text) = string_at(&mut caller, text, length) else {
                return;
            };
            match level {
                1 => log::error!("{text}"),
                2 => log::warn!("{text}"),
                4 => log::debug!("{text}"),
                _ => log::info!("{text}"),
            }
        },
    )?;

    Ok(())
}

/// Reads a string out of the caller's memory, or `None` if it is not there.
///
/// `None` rather than a trap: a plugin that passes a bad pointer to `log` has
/// a bug, and a bug in its logging must not be a bug in the window. The call
/// does nothing and the frame goes on.
fn string_at(caller: &mut Caller<'_, Registry>, pointer: i32, length: i32) -> Option<String> {
    String::from_utf8(bytes_at(caller, pointer, length)?).ok()
}

/// The same read, for the things that are not text.
///
/// Every bound the guest gave is checked against the guest's own memory here,
/// which is the one place a host function ever looks at it.
fn bytes_at(caller: &mut Caller<'_, Registry>, pointer: i32, length: i32) -> Option<Vec<u8>> {
    if length < 0 || length as u32 > MAX_ANSWER || pointer < 0 {
        return None;
    }
    let memory = caller.get_export(exports::MEMORY)?.into_memory()?;
    let data = memory.data(&caller);
    let start = pointer as usize;
    Some(
        data.get(start..start.checked_add(length as usize)?)?
            .to_vec(),
    )
}
