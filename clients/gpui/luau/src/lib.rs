//! One plugin pane's script in its own sandboxed Luau VM (ADR-0041, docs/plugin-runtime.md).
//!
//! The script returns a module `{ init, update, view }`. `init(previous)` makes the state (nil
//! for a new pane, the old state after a reload), `update(state, msg)` changes it (returning the
//! new state, or nil to keep the one it changed in place), and `view(state, data)` returns a tree
//! built with `ui.<Component>{ ... }`. `data.feed(name, params)` and `wheel.action(name, args)`
//! call the caller's [`Host`].
//!
//! `require("lib/ui")` runs the package's `lib/ui.luau` from `modules` (paths in the package; the
//! `.luau` may be left off) once per VM and returns what it returned. A path that leaves the
//! package, a missing file and a module that requires itself again before it returns each raise
//! an error. A reload takes the new modules and runs each again on its next `require`.
//!
//! The VM is interpreter only, its globals are frozen by `luaL_sandbox`, it has no `os` or `io`,
//! every entry into script code (a required module included) runs under a step budget, and the
//! VM's memory is capped while script code runs. Every failure comes back as a [`Failure`] and
//! leaves the last good module in place; the state keeps whatever the script changed in it
//! before failing. The module's fields are read raw, so a metatable on it runs no script code
//! outside the limits.

mod canvas_font;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::Rc;

use base64::Engine;
use mlua::{Function, Lua, MultiValue, Table, Value, VmState};
use serde_json::{Map, Value as Json};

pub use canvas_font::{CanvasFont, Design, Metrics};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes the VM may hold while script code runs.
    pub memory: usize,
    /// Interrupt safepoints (loop back edges and calls) one entry may pass.
    pub steps: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { memory: 32 << 20, steps: 10_000_000 }
    }
}

/// A feed as the script reads it: its value, and whether the backend that publishes it is down,
/// so the value is the last one it sent.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedRead {
    pub value: Json,
    pub stale: bool,
}

impl FeedRead {
    pub fn new(value: Json) -> Self {
        Self { value, stale: false }
    }
}

type FeedFn = dyn Fn(&str, &Json) -> FeedRead;
type ActionFn = dyn Fn(&str, &Json);
type MeasureFn = dyn Fn(&str, &CanvasFont) -> Metrics;

/// What the script reaches outside its VM.
pub struct Host {
    /// A feed now, for `data.feed(name, params)`, which returns `value, stale`.
    pub feed: Box<FeedFn>,
    /// `wheel.action(name, args)`.
    pub action: Box<ActionFn>,
    /// `ctx:measureText(text)` in the font `ctx.font` names.
    pub measure: Box<MeasureFn>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            feed: Box::new(|_, _| FeedRead::new(Json::Null)),
            action: Box::new(|_, _| {}),
            measure: Box::new(|text, font| font.measure(text)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The source did not compile.
    Syntax(String),
    /// The script raised an error.
    Runtime(String),
    /// The script ran past its step budget.
    StepBudget(String),
    /// The script asked for memory past its limit.
    Memory(String),
    /// The module is not a table with a `view` function.
    Module(String),
    /// `view` returned something that is not a component tree.
    View(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Self::Syntax(m) | Self::Runtime(m) | Self::StepBudget(m) | Self::Memory(m) | Self::Module(m) | Self::View(m) => m,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Failure {}

/// What stopped a conversion to JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLarge {
    pub canvas: bool,
}

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.canvas {
            f.write_str("a Canvas holds more than 2,000,000 values in its commands")
        } else {
            write!(f, "more than {MAX_NODES} values (a shared or cyclic table?)")
        }
    }
}

impl std::error::Error for TooLarge {}

/// One node of the tree a pane script's `view` returns: a component under its own name, its
/// properties (`modifiers` among them, ADR-0038), and the nodes in its array part.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneNode {
    pub component: String,
    /// Every property but `component` and `children`. A property may hold nodes anywhere inside
    /// it (a Button's `child`, a Table cell's `view`).
    pub props: Map<String, Json>,
    pub children: Vec<PaneNode>,
}

impl PaneNode {
    /// The node `value` describes, or None when it is not an object with a `component` name, or
    /// one of its children is not.
    pub fn from_json(value: Json) -> Option<Self> {
        let Json::Object(mut fields) = value else { return None };
        let Some(Json::String(component)) = fields.remove("component") else { return None };
        let children = match fields.remove("children") {
            None => Vec::new(),
            Some(Json::Array(list)) => list.into_iter().map(Self::from_json).collect::<Option<_>>()?,
            Some(_) => return None,
        };
        Some(Self { component, props: fields, children })
    }

    /// The node as the JSON the script built: `component`, the props, and `children` when there are any.
    pub fn to_json(&self) -> Json {
        let mut fields = self.props.clone();
        fields.insert("component".into(), Json::String(self.component.clone()));
        if !self.children.is_empty() {
            fields.insert("children".into(), Json::Array(self.children.iter().map(Self::to_json).collect()));
        }
        Json::Object(fields)
    }
}

const MAX_DEPTH: usize = 200;
/// Values one conversion to JSON may visit. A table reached twice is converted twice, so a shared
/// or cyclic table would otherwise grow without bound; the conversion runs outside the step budget.
const MAX_NODES: usize = 100_000;
/// Values one Canvas's command list may hold, counted apart from the view's: 250,000 commands of
/// at most 8 values each always fit, so only a list the script changed after `ui.Canvas` returned
/// (a shared or cyclic table appended) reaches it.
const MAX_CANVAS_VALUES: usize = 2_000_001;
/// Entries the measure cache holds before it is cleared.
const MAX_MEASURE_CACHE: usize = 5_000;
/// Steps one uncached `measureText` call charges to the budget.
const MEASURE_COST: u64 = 1_000;

/// What the VM's callbacks share with the runtime.
struct Shared {
    limits: Limits,
    /// 1 while script code runs under [`PaneRuntime::call`]; a host call lifts it.
    enforcing: Cell<bool>,
    steps: Cell<u64>,
    hit_budget: Cell<bool>,
    host: Host,
    modules: RefCell<HashMap<String, String>>,
    /// What a module's chunk name starts with: the pane chunk's plugin, `<plugin>/`.
    module_prefix: String,
    measure_cache: RefCell<HashMap<String, Metrics>>,
}

impl Shared {
    fn enforce(&self, lua: &Lua, on: bool) {
        self.enforcing.set(on);
        // A limit of 0 is none, so host calls and the runtime's own pushes cannot fail.
        let _ = lua.set_memory_limit(if on { self.limits.memory } else { 0 });
    }
}

/// Lifts the limits while a host call runs, and puts them back after.
struct Lifted<'a> {
    lua: &'a Lua,
    shared: &'a Shared,
    was: bool,
}

impl<'a> Lifted<'a> {
    fn new(lua: &'a Lua, shared: &'a Shared) -> Self {
        let was = shared.enforcing.get();
        if was {
            shared.enforce(lua, false);
        }
        Self { lua, shared, was }
    }
}

impl Drop for Lifted<'_> {
    fn drop(&mut self) {
        if self.was {
            self.shared.enforce(self.lua, true);
        }
    }
}

/// Wraps a host function that returns `ok, ...` so its failure is raised as a plain string, as a
/// C host function's `lua_error` raises it.
const HOST_WRAPPER: &str = r#"
local host, results = ...
if results == 0 then
    return function(...)
        local ok, message = host(...)
        if not ok then error(message, 0) end
    end
end
return function(...)
    local ok, a, b = host(...)
    if not ok then error(a, 0) end
    return a, b
end
"#;

pub struct PaneRuntime {
    lua: Lua,
    shared: Rc<Shared>,
    chunk_name: String,
    module: Table,
    state: Value,
    /// `require`'s table: `loaded` (path to `{ value = <what the module returned> }`) and
    /// `loading` (the paths whose module is running now).
    require: Table,
    /// The canvas sizes the client reported, `key` to `{ w, h }`; the `ui` prelude reads it.
    sizes: Table,
    /// A weak-keyed set of the command lists the `ui` prelude recorded; [`Self::to_json`] counts
    /// each one's values apart from the view's.
    canvases: Table,
    /// `data.feed`, wrapped.
    feed: Function,
}

impl PaneRuntime {
    pub fn new(source: &str, modules: HashMap<String, String>, chunk_name: &str, limits: Limits, host: Host) -> Result<Self, Failure> {
        let module_prefix = match chunk_name.split_once('/') {
            Some((plugin, _)) => format!("{plugin}/"),
            None => String::new(),
        };
        let shared = Rc::new(Shared {
            limits,
            enforcing: Cell::new(false),
            steps: Cell::new(0),
            hit_budget: Cell::new(false),
            host,
            modules: RefCell::new(modules),
            module_prefix,
            measure_cache: RefCell::new(HashMap::new()),
        });
        let lua = Lua::new();
        let (require, sizes, canvases, feed) = prepare_globals(&lua, &shared).map_err(|e| Failure::Runtime(message(&e)))?;
        let mut runtime = Self {
            lua,
            shared,
            chunk_name: format!("={chunk_name}"),
            module: require.clone(),
            state: Value::Nil,
            require,
            sizes,
            canvases,
            feed,
        };
        runtime.module = runtime.load(source)?;
        runtime.state = runtime.init_state(&runtime.module.clone(), None)?;
        Ok(runtime)
    }

    /// Delivers `msg` to `update`; a module without `update` ignores it.
    pub fn send(&mut self, msg: &Json) -> Result<(), Failure> {
        let Some(update) = self.function("update") else { return Ok(()) };
        let msg = self.push(msg);
        let result = self.call(&update, MultiValue::from_vec(vec![self.state.clone(), msg]))?;
        if !result.is_nil() {
            self.state = result;
        }
        Ok(())
    }

    /// The tree `view(state, data)` returns. `data`'s fields reach the script as a table, beside
    /// `data.feed`.
    pub fn view(&mut self, data: &Json) -> Result<PaneNode, Failure> {
        let Some(view) = self.function("view") else { return Err(Failure::Module("the module has no view function".into())) };
        let table = match data {
            Json::Object(_) => self.push(data),
            _ => Value::Table(self.lua.create_table().map_err(|e| Failure::Runtime(message(&e)))?),
        };
        if let Value::Table(t) = &table {
            t.set("feed", self.feed.clone()).map_err(|e| Failure::Runtime(message(&e)))?;
        }
        let result = self.call(&view, MultiValue::from_vec(vec![self.state.clone(), table]))?;
        let value = self.to_json(&result).map_err(|e| Failure::View(format!("view returned {e}")))?;
        PaneNode::from_json(value).ok_or_else(|| Failure::View("view returned something that is not a component tree".into()))
    }

    /// Runs `source` as the pane's module, with `modules` for its `require`, and hands the current
    /// state to its `init`. A failure keeps the previous module, modules and state.
    pub fn reload(&mut self, source: &str, modules: HashMap<String, String>) -> Result<(), Failure> {
        let old_modules = self.shared.modules.replace(modules);
        let old_loaded = self.swap_loaded(None);
        // A module that yielded while loading and was never resumed must not read as a cycle.
        if let Ok(loading) = self.require.raw_get::<Table>("loading") {
            let _ = loading.clear();
        }
        let loaded = self.load(source).and_then(|module| {
            let state = self.init_state(&module, Some(&self.state.clone()))?;
            Ok((module, state))
        });
        match loaded {
            Ok((module, state)) => {
                self.module = module;
                self.state = state;
                Ok(())
            }
            Err(e) => {
                self.shared.modules.replace(old_modules);
                self.swap_loaded(Some(old_loaded));
                Err(e)
            }
        }
    }

    /// The state, as JSON.
    pub fn state(&self) -> Result<Json, TooLarge> {
        self.to_json(&self.state)
    }

    /// Stores the size the client measured for the Canvas named `key`, which the next `view` hands
    /// to its `draw`. Returns whether it changed by 0.5 pt or more in either direction.
    pub fn set_canvas_size(&self, key: &str, width: f64, height: f64) -> bool {
        if let Ok(Value::Table(old)) = self.sizes.raw_get::<Value>(key) {
            let w: f64 = old.raw_get("w").unwrap_or(0.0);
            let h: f64 = old.raw_get("h").unwrap_or(0.0);
            if (w - width).abs() < 0.5 && (h - height).abs() < 0.5 {
                return false;
            }
        }
        if let Ok(size) = self.lua.create_table() {
            let _ = size.raw_set("w", width);
            let _ = size.raw_set("h", height);
            let _ = self.sizes.raw_set(key, size);
        }
        true
    }

    /// Bytes the VM holds.
    pub fn memory_used(&self) -> usize {
        self.lua.used_memory()
    }

    // Loading

    /// Runs `source` and returns the module it returns.
    fn load(&self, source: &str) -> Result<Table, Failure> {
        let chunk = compile(&self.lua, source, &self.chunk_name).map_err(|e| Failure::Syntax(message(&e)))?;
        let Value::Table(module) = self.call(&chunk, MultiValue::new())? else {
            return Err(Failure::Module("the script must return a table with init, update and view".into()));
        };
        if !matches!(module.raw_get::<Value>("view"), Ok(Value::Function(_))) {
            return Err(Failure::Module("the module has no view function".into()));
        }
        Ok(module)
    }

    /// Calls the module's `init` with `previous`, or nil, and returns the new state. A module
    /// without `init` starts from an empty table.
    fn init_state(&self, module: &Table, previous: Option<&Value>) -> Result<Value, Failure> {
        let empty = || self.lua.create_table().map(Value::Table).map_err(|e| Failure::Runtime(message(&e)));
        let Ok(Value::Function(init)) = module.raw_get::<Value>("init") else {
            return match previous {
                Some(p) => Ok(p.clone()),
                None => empty(),
            };
        };
        let state = self.call(&init, MultiValue::from_vec(vec![previous.cloned().unwrap_or(Value::Nil)]))?;
        if state.is_nil() { empty() } else { Ok(state) }
    }

    /// The module's function `name`, or None when the module has none.
    fn function(&self, name: &str) -> Option<Function> {
        match self.module.raw_get::<Value>(name) {
            Ok(Value::Function(f)) => Some(f),
            _ => None,
        }
    }

    /// Puts a new `loaded` table in `require`'s table, or `table`, and returns the one it replaced.
    fn swap_loaded(&self, table: Option<Table>) -> Table {
        let old: Table = self.require.raw_get("loaded").expect("require's table holds loaded");
        let new = table.unwrap_or_else(|| self.lua.create_table().expect("a table"));
        let _ = self.require.raw_set("loaded", new);
        old
    }

    /// Calls `f` with the limits on and returns its first result.
    fn call(&self, f: &Function, args: MultiValue) -> Result<Value, Failure> {
        let limits = self.shared.limits;
        // The incremental collector's threshold can sit far above the live heap, so garbage from
        // earlier entries would count against this one.
        if self.lua.used_memory() > limits.memory / 2 {
            let _ = self.lua.gc_collect();
        }
        self.shared.steps.set(0);
        self.shared.hit_budget.set(false);
        self.shared.enforce(&self.lua, true);
        let result = f.call::<Value>(args);
        self.shared.enforce(&self.lua, false);
        result.map_err(|e| {
            let text = message(&e);
            // The budget stays spent once hit, so a script that caught the budget's error is
            // stopped by it again; a memory error caught by the script and replaced is the
            // script's own error.
            if self.shared.hit_budget.get() {
                Failure::StepBudget(text)
            } else if is_memory(&e) {
                let _ = self.lua.gc_collect();
                Failure::Memory(format!("the script ran past its memory limit of {} bytes", limits.memory))
            } else {
                Failure::Runtime(text)
            }
        })
    }

    // Values

    fn push(&self, value: &Json) -> Value {
        push(&self.lua, value, 0).unwrap_or(Value::Nil)
    }

    /// The Lua value as JSON. A table whose keys are exactly 1...n is an array, as is an empty
    /// table; any other table is an object, its number keys written as strings. Functions and
    /// other values are null.
    fn to_json(&self, value: &Value) -> Result<Json, TooLarge> {
        let mut visited = 0;
        to_json(&self.canvases, value, 0, &mut visited, MAX_NODES, false)
    }
}

/// `ui`, whose fields build components under the names they are read by, `require` and `wheel`,
/// then the sandbox that freezes them with every library.
fn prepare_globals(lua: &Lua, shared: &Rc<Shared>) -> mlua::Result<(Table, Table, Table, Function)> {
    let globals = lua.globals();
    globals.raw_set("os", Value::Nil)?;
    let interrupt = shared.clone();
    lua.set_interrupt(move |lua| {
        if !interrupt.enforcing.get() {
            return Ok(VmState::Continue);
        }
        let steps = interrupt.steps.get().saturating_add(1);
        interrupt.steps.set(steps);
        if steps <= interrupt.limits.steps {
            return Ok(VmState::Continue);
        }
        // The count stays past the budget, so a script that catches this with pcall is stopped
        // again at its next safepoint.
        interrupt.hit_budget.set(true);
        let at = lua
            .inspect_stack(1, |d| {
                let line = d.current_line()?;
                Some(format!("{}:{line}: ", d.source().short_src?))
            })
            .flatten()
            .unwrap_or_default();
        Err(mlua::Error::runtime(format!("{at}the script ran past its step budget of {}", interrupt.limits.steps)))
    });

    let sizes = lua.create_table()?;
    let canvases = lua.create_table()?;
    let measure = {
        let shared = shared.clone();
        lua.create_function(move |lua, (text, font): (Value, Value)| {
            let _lifted = Lifted::new(lua, &shared);
            let (Value::String(text), Value::String(font)) = (text, font) else {
                return Err(mlua::Error::runtime("ctx:measureText expects text"));
            };
            let (text, font) = (text.to_string_lossy(), font.to_string_lossy());
            let key = format!("{font}\0{text}");
            let hit = shared.measure_cache.borrow().get(&key).copied();
            let m = match hit {
                Some(m) => m,
                None => {
                    shared.steps.set(shared.steps.get().wrapping_add(MEASURE_COST));
                    let m = (shared.host.measure)(&text, &CanvasFont::parse(&font));
                    let mut cache = shared.measure_cache.borrow_mut();
                    if cache.len() >= MAX_MEASURE_CACHE {
                        cache.clear();
                    }
                    cache.insert(key, m);
                    m
                }
            };
            Ok((m.width, m.ascent, m.descent))
        })?
    };
    let ui: Table = compile(lua, include_str!("ui.luau"), "=ui")?.call((measure, sizes.clone(), canvases.clone()))?;
    globals.raw_set("ui", ui)?;

    let require = lua.create_table()?;
    require.raw_set("loaded", lua.create_table()?)?;
    require.raw_set("loading", lua.create_table()?)?;
    let load_module = {
        let shared = shared.clone();
        lua.create_function(move |lua, name: Value| {
            let _lifted = Lifted::new(lua, &shared);
            let Value::String(name) = name else { return Ok((Value::Nil, Value::Nil)) };
            let name = name.to_string_lossy();
            let Some(source) = shared.modules.borrow().get(&name).cloned() else { return Ok((Value::Nil, Value::Nil)) };
            match compile(lua, &source, &format!("={}{name}", shared.module_prefix)) {
                Ok(f) => Ok((Value::Function(f), Value::Nil)),
                Err(e) => Ok((Value::Nil, Value::String(lua.create_string(message(&e))?))),
            }
        })?
    };
    let require_fn: Function = compile(lua, include_str!("require.luau"), "=require")?.call((load_module, require.clone()))?;
    globals.raw_set("require", require_fn)?;

    let wrapper = compile(lua, HOST_WRAPPER, "=host")?;
    let feed_host = {
        let shared = shared.clone();
        let canvases = canvases.clone();
        lua.create_function(move |lua, args: MultiValue| {
            let _lifted = Lifted::new(lua, &shared);
            let (name, params) = match host_args(&canvases, &args) {
                Ok(a) => a,
                Err(m) => return Ok(MultiValue::from_vec(vec![Value::Boolean(false), Value::String(lua.create_string(m)?)])),
            };
            let read = (shared.host.feed)(&name, &params);
            Ok(MultiValue::from_vec(vec![Value::Boolean(true), push(lua, &read.value, 0)?, Value::Boolean(read.stale)]))
        })?
    };
    let feed: Function = wrapper.call((feed_host, 2))?;
    let action_host = {
        let shared = shared.clone();
        let canvases = canvases.clone();
        lua.create_function(move |lua, args: MultiValue| {
            let _lifted = Lifted::new(lua, &shared);
            match host_args(&canvases, &args) {
                Ok((name, params)) => {
                    (shared.host.action)(&name, &params);
                    Ok((true, Value::Nil))
                }
                Err(m) => Ok((false, Value::String(lua.create_string(m)?))),
            }
        })?
    };
    let wheel = lua.create_table()?;
    wheel.raw_set("action", wrapper.call::<Function>((action_host, 0))?)?;
    globals.raw_set("wheel", wheel)?;
    // mlua adds `loadstring` to Luau's libraries; WheelLuau's `luaL_openlibs` VM has none.
    globals.raw_set("loadstring", Value::Nil)?;
    lua.sandbox(true)?;
    Ok((require, sizes, canvases, feed))
}

/// A host call's `(name, params)`: the name must be a string, and params, when given, convert.
fn host_args(canvases: &Table, args: &MultiValue) -> Result<(String, Json), String> {
    let Some(Value::String(name)) = args.front() else { return Err("the first argument must be a name".into()) };
    let params = match args.get(1) {
        Some(v) => {
            let mut visited = 0;
            to_json(canvases, v, 0, &mut visited, MAX_NODES, false).map_err(|e| format!("the arguments are {e}"))?
        }
        None => Json::Null,
    };
    Ok((name.to_string_lossy(), params))
}

/// Compiles `source` into a function whose globals are a table of its own over the frozen ones,
/// as `luaL_sandboxthread` gives a thread.
fn compile(lua: &Lua, source: &str, name: &str) -> mlua::Result<Function> {
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", lua.globals())?;
    meta.set_readonly(true);
    env.set_metatable(Some(meta))?;
    // Text only: mlua would otherwise run a source that starts with a bytecode version byte as
    // unverified Luau bytecode, which WheelLuau's `luau_compile` never does.
    lua.load(source).set_name(name).set_mode(mlua::chunk::ChunkMode::Text).set_environment(env).into_function()
}

/// The text a failure carries: the script's own message, without the wrapping mlua adds.
fn message(e: &mlua::Error) -> String {
    match e {
        // mlua's call handler appends Luau's traceback to a runtime error; WheelLuau's message has none.
        mlua::Error::RuntimeError(m) => m.rfind("\nstack traceback:\n").map_or(m.as_str(), |at| &m[..at]).to_owned(),
        mlua::Error::MemoryError(m) => m.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::CallbackError { cause, .. } => message(cause),
        mlua::Error::WithContext { cause, .. } => message(cause),
        other => other.to_string(),
    }
}

fn is_memory(e: &mlua::Error) -> bool {
    match e {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => is_memory(cause),
        _ => false,
    }
}

fn push(lua: &Lua, value: &Json, depth: usize) -> mlua::Result<Value> {
    Ok(match value {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(list) => {
            let t = lua.create_table_with_capacity(list.len(), 0)?;
            if depth < MAX_DEPTH {
                for (i, item) in list.iter().enumerate() {
                    t.raw_set(i + 1, push(lua, item, depth + 1)?)?;
                }
            }
            Value::Table(t)
        }
        Json::Object(fields) => {
            let t = lua.create_table_with_capacity(0, fields.len())?;
            if depth < MAX_DEPTH {
                for (key, item) in fields {
                    t.raw_set(key.as_str(), push(lua, item, depth + 1)?)?;
                }
            }
            Value::Table(t)
        }
    })
}

fn number(n: f64) -> Json {
    serde_json::Number::from_f64(n).map(Json::Number).unwrap_or(Json::Null)
}

/// A number key as an object key: `3`, not `3.0`.
fn key(n: f64) -> String {
    if n == n.round() && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

fn to_json(canvases: &Table, value: &Value, depth: usize, visited: &mut usize, limit: usize, in_canvas: bool) -> Result<Json, TooLarge> {
    *visited += 1;
    if *visited > limit {
        return Err(TooLarge { canvas: in_canvas });
    }
    Ok(match value {
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => number(*i as f64),
        Value::Number(n) => number(*n),
        Value::String(s) => Json::String(s.to_string_lossy()),
        Value::Buffer(b) => Json::String(base64::engine::general_purpose::STANDARD.encode(b.to_vec())),
        Value::Table(t) => {
            if depth >= MAX_DEPTH {
                return Err(TooLarge { canvas: false });
            }
            if !in_canvas && canvases.raw_get::<bool>(t.clone()).unwrap_or(false) {
                let mut own = 0;
                return to_json(canvases, value, depth, &mut own, MAX_CANVAS_VALUES, true);
            }
            let mut by_index: BTreeMap<i64, Json> = BTreeMap::new();
            let mut fields: Map<String, Json> = Map::new();
            for pair in t.pairs::<Value, Value>() {
                let Ok((k, v)) = pair else { continue };
                let item = to_json(canvases, &v, depth + 1, visited, limit, in_canvas)?;
                let n = match k {
                    Value::Integer(i) => i as f64,
                    Value::Number(n) => n,
                    Value::String(s) => {
                        fields.insert(s.to_string_lossy(), item);
                        continue;
                    }
                    _ => continue,
                };
                if n >= 1.0 && n == n.round() && n <= f64::from(i32::MAX) {
                    by_index.insert(n as i64, item.clone());
                }
                fields.insert(key(n), item);
            }
            if by_index.len() == fields.len() && by_index.keys().copied().eq(1..=by_index.len() as i64) {
                Json::Array(by_index.into_values().collect())
            } else {
                Json::Object(fields)
            }
        }
        _ => Json::Null,
    })
}
