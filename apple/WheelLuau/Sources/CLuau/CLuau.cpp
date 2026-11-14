// The pieces of the runtime that may raise a Luau error, kept in C++ so the exception that
// carries it never unwinds a Swift frame.
#include "CLuau.h"

#include <stdlib.h>

static WheelLuauLimits *limitsOf(lua_State *L) {
    return static_cast<WheelLuauLimits *>(lua_callbacks(L)->userdata);
}

static void *limitedAlloc(void *ud, void *ptr, size_t osize, size_t nsize) {
    auto *limits = static_cast<WheelLuauLimits *>(ud);
    if (nsize == 0) {
        free(ptr);
        limits->used -= osize;
        return nullptr;
    }
    if (limits->enforcing && nsize > osize && limits->used - osize + nsize > limits->memoryLimit) {
        limits->hitMemory = 1;
        return nullptr;
    }
    void *grown = realloc(ptr, nsize);
    if (grown == nullptr) return nullptr;
    limits->used = limits->used - osize + nsize;
    return grown;
}

static void budgetInterrupt(lua_State *L, int gc) {
    if (gc >= 0) return;  // a GC step, not a safepoint in script code
    WheelLuauLimits *limits = limitsOf(L);
    if (!limits->enforcing || ++limits->steps <= limits->stepBudget) return;
    // The count stays past the budget, so a script that catches this with pcall is stopped
    // again at its next safepoint.
    limits->hitBudget = 1;
    luaL_error(L, "the script ran past its step budget of %llu", limits->stepBudget);
}

extern "C" lua_State *wheel_luau_newstate(WheelLuauLimits *limits) {
    lua_State *L = lua_newstate(limitedAlloc, limits);
    if (L == nullptr) return nullptr;
    lua_callbacks(L)->userdata = limits;
    lua_callbacks(L)->interrupt = budgetInterrupt;
    return L;
}

extern "C" int wheel_luau_pcall(lua_State *L, int nargs, int nresults) {
    WheelLuauLimits *limits = limitsOf(L);
    // The allocator cannot collect, and the incremental collector's threshold can sit far above
    // the live heap, so garbage from earlier entries would count against this one.
    if (limits->used > limits->memoryLimit / 2) lua_gc(L, LUA_GCCOLLECT, 0);
    limits->steps = 0;
    limits->hitBudget = 0;
    limits->hitMemory = 0;
    limits->enforcing = 1;
    int status = lua_pcall(L, nargs, nresults, 0);
    limits->enforcing = 0;
    return status;
}

static int hostTrampoline(lua_State *L) {
    WheelLuauLimits *limits = limitsOf(L);
    int kind = static_cast<int>(lua_tointeger(L, lua_upvalueindex(1)));
    int was = limits->enforcing;
    limits->enforcing = 0;
    int results = limits->hostCall(limits->host, L, kind);
    limits->enforcing = was;
    if (results < 0) lua_error(L);  // the host pushed its message
    return results;
}

extern "C" void wheel_luau_pushhost(lua_State *L, int kind) {
    lua_pushinteger(L, kind);
    lua_pushcclosure(L, hostTrampoline, "host", 1);
}
