//! Common policy for game and editor Luau interpreters.

#![forbid(unsafe_code)]

use mlua::{Lua, LuaOptions, Result, StdLib, Value};

pub fn luau() -> Result<Lua> {
    Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::BUFFER | StdLib::BIT,
        LuaOptions::default(),
    )
}

pub fn seal(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    // The host owns code loading, scheduling, and error handling for quotas.
    for name in [
        "require",
        "loadstring",
        "getfenv",
        "setfenv",
        "collectgarbage",
        "pcall",
        "xpcall",
        "coroutine",
        "print",
    ] {
        globals.raw_set(name, Value::Nil)?;
    }
    lua.sandbox(true)
}
