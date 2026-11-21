use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{ffi, Error, Lua, Result, VmState};

use crate::LuauConfig;

// Luau reserves category 0 for VM-owned allocations.
pub(crate) const LAST_CATEGORY: i32 = 255;

pub(crate) struct Quota {
    max_memory: usize,
    global_limit: usize,
    frame_budget: Duration,
    active: Rc<Cell<Option<Execution>>>,
}

#[derive(Clone, Copy)]
struct Execution {
    start: Instant,
    budget: Duration,
    category: i32,
}

impl Quota {
    pub(crate) fn new(lua: &Lua, config: LuauConfig) -> Result<Self> {
        let frame_budget = Duration::try_from_secs_f64(config.max_cpu_seconds_per_frame)
            .map_err(|_| Error::runtime("invalid Luau execution budget"))?;
        if frame_budget.is_zero() || config.max_memory_bytes == 0 {
            return Err(Error::runtime("Luau quotas must be positive"));
        }
        let max_memory = config.max_memory_bytes;
        let global_limit =
            4 * 1024 * 1024 + max_memory.saturating_mul(16).min(isize::MAX as usize / 2);
        // mlua installs its Rust allocator as lua_newstate's allocation callback.
        // Its hard ceiling also covers Luau pool pages and VM-owned allocations.
        lua.set_memory_limit(global_limit)?;

        let active = Rc::new(Cell::new(None::<Execution>));
        let interrupt = Rc::clone(&active);
        lua.set_interrupt(move |lua| {
            if let Some(execution) = interrupt.get() {
                if execution.start.elapsed() >= execution.budget {
                    return Err(Error::runtime("Luau execution quota exceeded"));
                }
                if used_by(lua, execution.category) > max_memory {
                    return Err(Error::MemoryError(
                        "Luau script memory quota exceeded".into(),
                    ));
                }
            }
            Ok(VmState::Continue)
        });

        Ok(Self {
            max_memory,
            global_limit,
            frame_budget,
            active,
        })
    }

    pub(crate) fn frame_budget(&self) -> Duration {
        self.frame_budget
    }

    pub(crate) fn max_memory(&self) -> usize {
        self.max_memory
    }

    pub(crate) fn used_by(&self, lua: &Lua, category: i32) -> usize {
        used_by(lua, category)
    }

    pub(crate) fn run<T>(
        &self,
        lua: &Lua,
        category: i32,
        remaining: &mut Duration,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut used = used_by(lua, category);
        if used >= self.max_memory {
            lua.gc_collect()?;
            used = used_by(lua, category);
        }
        if used >= self.max_memory {
            return Err(Error::MemoryError(
                "Luau script memory quota exceeded".into(),
            ));
        }

        // A tighter allocator ceiling bounds large allocations between interrupt
        // checks. Logical category accounting also covers objects in shared pages.
        let headroom = self.max_memory - used;
        let limit = lua
            .used_memory()
            .saturating_add(headroom)
            .min(self.global_limit);
        let previous_limit = lua.set_memory_limit(limit)?;
        let execution = Execution {
            start: Instant::now(),
            budget: *remaining,
            category,
        };
        self.active.set(Some(execution));
        lua.exec_raw_lua(|raw| set_category(raw.state(), category));
        let guard = ExecutionScope {
            lua,
            active: &self.active,
            previous_limit,
        };
        let result = action();
        let elapsed = execution.start.elapsed();
        *remaining = remaining.saturating_sub(elapsed);
        drop(guard);

        let value = result?;
        if used_by(lua, category) > self.max_memory {
            return Err(Error::MemoryError(
                "Luau script memory quota exceeded".into(),
            ));
        }
        if elapsed >= execution.budget {
            return Err(Error::runtime("Luau execution quota exceeded"));
        }
        Ok(value)
    }
}

struct ExecutionScope<'a> {
    lua: &'a Lua,
    active: &'a Cell<Option<Execution>>,
    previous_limit: usize,
}

impl Drop for ExecutionScope<'_> {
    fn drop(&mut self) {
        self.lua.exec_raw_lua(|raw| set_category(raw.state(), 0));
        self.active.set(None);
        // This VM always uses the mlua allocator, so restoring its limit is infallible.
        let _ = self.lua.set_memory_limit(self.previous_limit);
    }
}

fn used_by(lua: &Lua, category: i32) -> usize {
    lua.exec_raw_lua(|raw| {
        // SAFETY: raw is a locked live VM and category is a valid Luau memory category.
        unsafe { ffi::lua_totalbytes(raw.state(), category) }
    })
}

pub(crate) fn set_category(state: *mut ffi::lua_State, category: i32) {
    // SAFETY: private callers pass a live state from a locked RawLua or a live Thread.
    // Categories are limited to the Luau range, including 0 for the host.
    unsafe { ffi::lua_setmemcat(state, category) }
}
