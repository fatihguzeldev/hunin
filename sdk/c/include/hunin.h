#ifndef HUNIN_H
#define HUNIN_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define HUNIN_ID_CAPACITY 128
/* Tokens are plain values: copy them into jobs to propagate across threads. */
typedef struct {
    char run_id[HUNIN_ID_CAPACITY], context_id[HUNIN_ID_CAPACITY], parent_id[HUNIN_ID_CAPACITY];
} hunin_context_t;
/* A live span owns its name. Call end exactly once before reusing its storage.
 */
typedef struct {
    char id[HUNIN_ID_CAPACITY];
    hunin_context_t context;
    char *name;
    uint64_t timestamp_us, started_us;
    unsigned long owner_pid;
    int ended;
} hunin_span_t;
int hunin_enabled(void);
int hunin_last_error(void);
int hunin_flush(void);
hunin_context_t hunin_context_new(const char *name);
hunin_context_t hunin_context_current_run(void);
void hunin_span_begin(hunin_span_t *span, const char *name, const hunin_context_t *parent);
void hunin_span_end(hunin_span_t *span);
hunin_context_t hunin_span_context(const hunin_span_t *span);
void hunin_counter(const hunin_context_t *context, const char *name, double value,
                   const char *unit);
void hunin_event(const hunin_context_t *context, const char *name);
void hunin_relation(const hunin_context_t *context, const char *from, const char *to,
                    const char *relation);
#ifdef __cplusplus
}
#endif
#endif
