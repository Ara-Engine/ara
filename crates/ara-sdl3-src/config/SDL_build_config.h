/* Ara's SDL configuration, based on SDL's zlib-licensed build_config templates.
 * Audio, native video/input, Vulkan surfaces and dialogs; no renderer/GPU/camera.
 * See ../vendor/SDL3-3.4.18/LICENSE.txt. */
#ifndef ARA_SDL_BUILD_CONFIG_H
#define ARA_SDL_BUILD_CONFIG_H
#include <SDL3/SDL_platform_defines.h>
#define SDL_GPU_DISABLED 1
#define SDL_RENDER_DISABLED 1
#define SDL_CAMERA_DISABLED 1
#define SDL_TRAY_DUMMY 1
#if defined(SDL_PLATFORM_WINDOWS)
#include "SDL_windows.h"
#elif defined(SDL_PLATFORM_ANDROID)
#include "SDL_android.h"
#elif defined(SDL_PLATFORM_LINUX)
#include "SDL_linux.h"
#else
#error TODO(D15): configure Apple SDL backends
#endif
#endif
