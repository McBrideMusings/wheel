#pragma once
#include <stddef.h>
#include "lua.h"
#include "lualib.h"
#include "../../../luau/Compiler/include/luacode.h"

#ifdef __cplusplus
extern "C" {
#endif

// The pseudo-indices lua.h writes as macros Swift cannot import.
static const int WHEEL_LUA_REGISTRYINDEX = LUA_REGISTRYINDEX;
static const int WHEEL_LUA_GLOBALSINDEX = LUA_GLOBALSINDEX;

/// A host function the script calls (`data.feed`, `wheel.action`). It reads its arguments from
/// the stack and pushes its results, and returns how many it pushed, or -1 after pushing an error
/// message. It must raise no Luau error itself (no luaL_check*), since a Luau error is a C++
/// exception and must never unwind a Swift frame; the memory limit is lifted while it runs, so
/// its pushes cannot fail.
typedef int (*WheelLuauHostCall)(void* host, lua_State* L, int kind);

/// One VM's limits and what tripped them. The caller owns it, and it must outlive the state.
typedef struct WheelLuauLimits {
    size_t memoryLimit;             // bytes the state may hold while script code runs
    size_t used;                    // bytes it holds now
    unsigned long long stepBudget;  // interrupt safepoints (loop back edges, calls) per entry
    unsigned long long steps;       // safepoints counted in the current entry
    int enforcing;                  // 1 while script code runs under wheel_luau_pcall
    int hitBudget;                  // set when the step budget stopped the current entry
    int hitMemory;                  // set when the memory limit refused an allocation
    void *host;                     // passed back to hostCall
    WheelLuauHostCall hostCall;
} WheelLuauLimits;

/// A state whose allocator counts into `limits` and refuses growth past `memoryLimit` while
/// enforcing, and whose interrupt callback stops script code after `stepBudget` safepoints.
lua_State *wheel_luau_newstate(WheelLuauLimits *limits);

/// lua_pcall with the limits enforced: the step count restarts, both flags clear, and
/// enforcing is on only for the call.
int wheel_luau_pcall(lua_State *L, int nargs, int nresults);

/// Pushes a C function that calls `limits->hostCall` with `kind`.
void wheel_luau_pushhost(lua_State *L, int kind);

#ifdef __cplusplus
}
#endif
