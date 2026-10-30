#define _GNU_SOURCE
#include <dlfcn.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#ifndef HUNIN_COUNT_MAX
#define HUNIN_COUNT_MAX 8192
#endif
#define SAFE_COUNT UINT64_C(9007199254740991)
#define NO_HOOK __attribute__((no_instrument_function))

typedef struct {
    _Atomic uintptr_t address;
    _Atomic uint64_t count;
} Entry;
static Entry entries[HUNIN_COUNT_MAX];
static _Atomic uint64_t dropped;
static _Atomic int overflow;
static _Atomic int enabled;
static pid_t owner;
static uint64_t started;

static uint64_t NO_HOOK epoch_us(void) {
    struct timespec now;
    clock_gettime(CLOCK_REALTIME, &now);
    return (uint64_t)now.tv_sec * UINT64_C(1000000) + (uint64_t)now.tv_nsec / 1000;
}
static void NO_HOOK child_disable(void) { atomic_store(&enabled, 0); }
__attribute__((constructor(101), no_instrument_function)) static void initialize(void) {
    owner = getpid();
    started = epoch_us();
    pthread_atfork(NULL, NULL, child_disable);
    atomic_store(&enabled, 1);
}

void NO_HOOK __cyg_profile_func_enter(void *function, void *caller) {
    (void)caller;
    if (!atomic_load_explicit(&enabled, memory_order_relaxed))
        return;
    uintptr_t key = (uintptr_t)function;
    size_t initial = (key >> 4) % HUNIN_COUNT_MAX;
    for (size_t distance = 0; distance < HUNIN_COUNT_MAX; ++distance) {
        Entry *entry = &entries[(initial + distance) % HUNIN_COUNT_MAX];
        uintptr_t found = atomic_load_explicit(&entry->address, memory_order_relaxed);
        if (!found) {
            uintptr_t empty = 0;
            atomic_compare_exchange_strong(&entry->address, &empty, key);
            found = atomic_load(&entry->address);
        }
        if (found == key) {
            uint64_t value = atomic_fetch_add_explicit(&entry->count, 1, memory_order_relaxed);
            if (value >= SAFE_COUNT)
                atomic_store(&overflow, 1);
            return;
        }
    }
    atomic_fetch_add(&dropped, 1);
}
void NO_HOOK __cyg_profile_func_exit(void *function, void *caller) {
    (void)function;
    (void)caller;
}

static int NO_HOOK selected(const char *name) {
    const char *list = getenv("HUNIN_COUNT_FUNCTIONS");
    if (!list || !*list)
        return 1;
    while (*list) {
        const char *end = strchr(list, '\n');
        size_t length = end ? (size_t)(end - list) : strlen(list);
        if (strlen(name) == length && memcmp(name, list, length) == 0)
            return 1;
        if (!end)
            break;
        list = end + 1;
    }
    return 0;
}

static int NO_HOOK escape(char *out, size_t capacity, const char *value) {
    size_t used = 0;
    for (const unsigned char *p = (const unsigned char *)value; *p; ++p) {
        if (used + 7 >= capacity)
            return 0;
        if (*p == '"' || *p == '\\') {
            out[used++] = '\\';
            out[used++] = (char)*p;
        } else if (*p < 32 || *p >= 127) {
            used += (size_t)snprintf(out + used, capacity - used, "\\u%04x", *p);
        } else {
            out[used++] = (char)*p;
        }
    }
    out[used] = '\0';
    return 1;
}

typedef struct {
    FILE *file;
    uint64_t bytes, records, hash, maximum;
    int truncated, failed;
} Output;
static void NO_HOOK append(Output *output, const char *line, int sentinel) {
    size_t length = strlen(line);
    if (output->bytes + length + (sentinel ? 0 : 768) > output->maximum) {
        output->truncated = 1;
        return;
    }
    if (fwrite(line, 1, length, output->file) != length) {
        output->failed = 1;
        return;
    }
    for (size_t i = 0; i < length; ++i) {
        output->hash ^= (unsigned char)line[i];
        output->hash *= UINT64_C(0x100000001b3);
    }
    output->bytes += length;
    ++output->records;
}

__attribute__((destructor(101), no_instrument_function)) static void finish(void) {
    atomic_store(&enabled, 0);
    if (!owner || owner != getpid())
        return;
    const char *path = getenv("HUNIN_COUNT_OUTPUT");
    const char *receipt = getenv("HUNIN_COUNT_COMPLETION");
    const char *run = getenv("HUNIN_COUNT_RUN_ID");
    const char *maximum = getenv("HUNIN_COUNT_MAX_BYTES");
    if (!path || !receipt || !run || !maximum)
        return;
    char escaped_run[8192];
    if (!escape(escaped_run, sizeof(escaped_run), run))
        return;
    Output output = {.file = fopen(path, "wx"),
                     .hash = UINT64_C(0xcbf29ce484222325),
                     .maximum = strtoull(maximum, NULL, 10)};
    if (!output.file || output.maximum < 1024 || output.maximum > 8388608)
        return;
    uint64_t timestamp = epoch_us();
    for (size_t i = 0; i < HUNIN_COUNT_MAX; ++i) {
        uintptr_t address = atomic_load(&entries[i].address);
        uint64_t count = atomic_load(&entries[i].count);
        if (!address || !count)
            continue;
        Dl_info symbol;
        memset(&symbol, 0, sizeof(symbol));
        dladdr((void *)address, &symbol);
        char fallback[32];
        snprintf(fallback, sizeof(fallback), "0x%" PRIxPTR, address);
        const char *name = symbol.dli_sname ? symbol.dli_sname : fallback;
        if (!selected(name))
            continue;
        char escaped_name[8192];
        if (strlen(name) > 1024 || !escape(escaped_name, sizeof(escaped_name), name)) {
            atomic_fetch_add(&dropped, 1);
            continue;
        }
        char line[24576];
        snprintf(line, sizeof(line),
                 "{\"schema_version\":1,\"kind\":\"metric\",\"id\":\"compiler-counts-%d:%zu\","
                 "\"run_id\":\"%s\",\"producer_id\":\"calls:%s\",\"pid\":%d,\"tid\":"
                 "\"process\",\"timestamp_us\":%" PRIu64
                 ",\"basis\":\"observed\",\"name\":\"function.calls\",\"value\":%" PRIu64
                 ",\"unit\":\"count\",\"scope\":\"process\",\"attributes\":{\"function\":{\"name\":"
                 "\"%s\",\"file\":null,\"line\":null},\"address\":\"%s\",\"count_semantics\":"
                 "\"starts\",\"method\":\"compiler-hooks\",\"clock\":\"unix_epoch\",\"clock_"
                 "alignment\":\"direct_epoch\",\"clock_source\":\"clock_gettime(CLOCK_REALTIME)\","
                 "\"collection_scope\":\"all_then_filter\",\"function_identity_kind\":\"linked_"
                 "symbol_or_address\",\"window_started_at_us\":%" PRIu64
                 ",\"window_finished_at_us\":%" PRIu64 "}}\n",
                 owner, i, escaped_run, escaped_run, owner, timestamp, count, escaped_name,
                 fallback, started, timestamp);
        append(&output, line, 0);
        if (output.truncated || output.failed)
            break;
    }
    int incomplete =
        atomic_load(&dropped) || atomic_load(&overflow) || output.truncated || output.failed;
    const char *status = incomplete ? "incomplete" : "complete";
    char sentinel[16384];
    snprintf(sentinel, sizeof(sentinel),
             "{\"schema_version\":1,\"kind\":\"event\",\"id\":\"compiler-counts-%d:complete\","
             "\"run_id\":\"%s\",\"producer_id\":\"calls:%s\",\"pid\":%d,\"tid\":"
             "\"process\",\"timestamp_us\":%" PRIu64
             ",\"basis\":\"observed\",\"name\":\"function.counts.complete\",\"attributes\":{"
             "\"status\":\"%s\",\"method\":\"compiler-hooks\",\"clock\":\"unix_epoch\",\"clock_"
             "alignment\":\"direct_epoch\",\"clock_source\":\"clock_gettime(CLOCK_REALTIME)\","
             "\"collection_scope\":\"all_then_filter\",\"window_started_at_us\":%" PRIu64
             ",\"window_finished_at_us\":%" PRIu64 "}}\n",
             owner, escaped_run, escaped_run, owner, timestamp, status, started, timestamp);
    append(&output, sentinel, 1);
    if (fclose(output.file) != 0)
        output.failed = 1;
    if (output.truncated || output.failed)
        status = "incomplete";
    FILE *manifest = fopen(receipt, "w");
    if (!manifest)
        return;
    fprintf(manifest,
            "{\"schema_version\":1,\"run_id\":\"%s\",\"method\":\"compiler-hooks\",\"status\":\"%"
            "s\",\"bytes\":%" PRIu64 ",\"records\":%" PRIu64 ",\"fnv1a64\":\"%016" PRIx64
            "\",\"dropped_events\":%" PRIu64
            ",\"counter_overflow\":%s,\"output_truncated\":%s,\"loss\":\"%s\"}",
            escaped_run, status, output.bytes, output.records, output.hash, atomic_load(&dropped),
            atomic_load(&overflow) ? "true" : "false", output.truncated ? "true" : "false",
            strcmp(status, "complete") == 0 ? "none_observed" : "incomplete");
    fclose(manifest);
}
