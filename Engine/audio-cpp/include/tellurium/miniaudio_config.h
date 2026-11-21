#pragma once

/* The asset worker decodes WAV and Vorbis. The mixer consumes PCM only. */
#define MA_NO_DECODING
#define MA_NO_ENCODING
#define MA_NO_RESOURCE_MANAGER
#include "miniaudio.h"
