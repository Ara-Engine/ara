/* Fixed-signature entry points keep Opus's variadic control API inside C. */
#include "opus.h"
int ara_opus_bitrate(OpusEncoder *encoder, int bitrate) {
    return opus_encoder_ctl(encoder, OPUS_SET_BITRATE(bitrate));
}
int ara_opus_lookahead(OpusEncoder *encoder, int *frames) {
    return opus_encoder_ctl(encoder, OPUS_GET_LOOKAHEAD(frames));
}
