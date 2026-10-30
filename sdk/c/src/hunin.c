#define _POSIX_C_SOURCE 200809L
#include "hunin.h"
#include <errno.h>
#include <inttypes.h>
#include <locale.h>
#ifdef __APPLE__
#include <xlocale.h>
#endif
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifdef _WIN32
#include <direct.h>
#include <io.h>
#include <process.h>
#include <windows.h>
static SRWLOCK writer_lock = SRWLOCK_INIT;
#define LOCK() AcquireSRWLockExclusive(&writer_lock)
#define UNLOCK() ReleaseSRWLockExclusive(&writer_lock)
#define PID() ((unsigned long)_getpid())
#define MAKE_DIR(path) _mkdir(path)
#else
#include <pthread.h>
#include <sys/stat.h>
#include <unistd.h>
static pthread_mutex_t writer_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_once_t fork_registration = PTHREAD_ONCE_INIT;
static void before_fork(void) { pthread_mutex_lock(&writer_lock); }
static void after_fork(void) { pthread_mutex_unlock(&writer_lock); }
static void register_fork(void) { pthread_atfork(before_fork, after_fork, after_fork); }
#define LOCK() (pthread_once(&fork_registration, register_fork), pthread_mutex_lock(&writer_lock))
#define UNLOCK() pthread_mutex_unlock(&writer_lock)
#define PID() ((unsigned long)getpid())
#define MAKE_DIR(path) mkdir(path, 0700)
#endif

static FILE *output;
static unsigned long owner_pid;
static uint64_t sequence;
static int failure;
static char producer[HUNIN_ID_CAPACITY];
static char run_id[HUNIN_ID_CAPACITY];
static int current_metric;
static uint64_t epoch_us(void) {
#ifdef _WIN32
    FILETIME ft;
    ULARGE_INTEGER ticks;
    GetSystemTimePreciseAsFileTime(&ft);
    ticks.LowPart = ft.dwLowDateTime;
    ticks.HighPart = ft.dwHighDateTime;
    return (ticks.QuadPart - 116444736000000000ULL) / 10;
#else
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec * 1000000 + (uint64_t)ts.tv_nsec / 1000;
#endif
}
static uint64_t monotonic_us(void) {
#ifdef _WIN32
    LARGE_INTEGER ticks, frequency;
    QueryPerformanceCounter(&ticks);
    QueryPerformanceFrequency(&frequency);
    return (uint64_t)((long double)ticks.QuadPart * 1000000 / frequency.QuadPart);
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000 + (uint64_t)ts.tv_nsec / 1000;
#endif
}
static void copy_id(char *target, const char *source) {
    snprintf(target, HUNIN_ID_CAPACITY, "%s", source ? source : "");
}
static int configured(void) {
    const char *run = getenv("HUNIN_RUN_ID"), *directory = getenv("HUNIN_RECORD_DIR");
    return run && *run && directory && *directory;
}
static void string(FILE *file, const char *text) {
    const unsigned char *p = (const unsigned char *)(text ? text : "");
    fputc('"', file);
    for (; *p; p++) {
        if (*p == '"' || *p == '\\') {
            fputc('\\', file);
            fputc(*p, file);
        } else if (*p < 32) {
            fprintf(file, "\\u%04x", *p);
        } else {
            fputc(*p, file);
        }
    }
    fputc('"', file);
}
static int make_directories(const char *path) {
    size_t length = strlen(path);
    char *copy = (char *)malloc(length + 1);
    size_t i, start = 1;
    if (!copy)
        return ENOMEM;
    memcpy(copy, path, length + 1);
#ifdef _WIN32
    DWORD attributes = GetFileAttributesA(path);
    if (attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY)) {
        free(copy);
        return 0;
    }
    if (length >= 2 && copy[1] == ':') {
        start = 2;
        if (length > 2 && (copy[2] == '/' || copy[2] == '\\'))
            start = 3;
    } else if (length > 1 && (copy[0] == '/' || copy[0] == '\\') &&
               (copy[1] == '/' || copy[1] == '\\')) {
        start = 2;
        if (length >= 8 && copy[2] == '?' && copy[3] == '\\' &&
            (copy[4] == 'U' || copy[4] == 'u') && (copy[5] == 'N' || copy[5] == 'n') &&
            (copy[6] == 'C' || copy[6] == 'c') && copy[7] == '\\')
            start = 8;
        for (int component = 0; component < 2 && start < length; component++) {
            while (start < length && copy[start] != '/' && copy[start] != '\\')
                start++;
            if (start < length)
                start++;
        }
    }
#endif
    for (i = start; i < length; i++) {
        if (copy[i] == '/' || copy[i] == '\\') {
            char separator = copy[i];
            copy[i] = 0;
            if (*copy && MAKE_DIR(copy) != 0 && errno != EEXIST) {
                int error = errno;
                free(copy);
                return error;
            }
            copy[i] = separator;
        }
    }
    if (MAKE_DIR(copy) != 0 && errno != EEXIST) {
        int error = errno;
        free(copy);
        return error;
    }
    free(copy);
    return 0;
}
static int prepare(void) {
    unsigned long pid = PID();
    const char *directory;
    char *path;
    size_t size;
    if (!configured())
        return 0;
    if (owner_pid != pid) {
        if (output)
            fclose(output);
        output = NULL;
        failure = 0;
        sequence = 0;
        owner_pid = pid;
        snprintf(producer, sizeof(producer), "c-%lu-%" PRIu64 "-%" PRIu64, pid, epoch_us(),
                 monotonic_us());
        copy_id(run_id, getenv("HUNIN_RUN_ID"));
    }
    if (failure)
        return 0;
    if (output)
        return 1;
    directory = getenv("HUNIN_RECORD_DIR");
    failure = make_directories(directory);
    if (failure)
        return 0;
    size = strlen(directory) + strlen(producer) + 32;
    path = (char *)malloc(size);
    if (!path) {
        failure = ENOMEM;
        return 0;
    }
#ifdef _WIN32
    /* Extended-length Windows paths do not normalize forward slashes. */
    snprintf(path, size, "%s%ssdk-%s.jsonl", directory,
             directory[strlen(directory) - 1] == '\\' ? "" : "\\", producer);
#else
    snprintf(path, size, "%s/sdk-%s.jsonl", directory, producer);
#endif
    output = fopen(path, "a");
    free(path);
    if (!output) {
        failure = errno;
        return 0;
    }
#ifndef _WIN32
    (void)fchmod(fileno(output), 0600);
#endif
    return 1;
}
static void identifier(char *id) {
    snprintf(id, HUNIN_ID_CAPACITY, "%s:%" PRIu64, producer, ++sequence);
}
static int begin(const char *kind, char *id, const hunin_context_t *context, uint64_t timestamp) {
    if (!prepare())
        return 0;
    current_metric = strcmp(kind, "metric") == 0;
    if (!*id)
        identifier(id);
    fprintf(output, "{\"schema_version\":1,\"kind\":");
    string(output, kind);
    fputs(",\"id\":", output);
    string(output, id);
    fputs(",\"run_id\":", output);
    string(output, run_id);
    fputs(",\"producer_id\":", output);
    string(output, producer);
    fprintf(output, ",\"pid\":%lu,\"tid\":\"", PID());
#ifdef _WIN32
    fprintf(output, "%lu", (unsigned long)GetCurrentThreadId());
#else
    fprintf(output, "%" PRIuPTR, (uintptr_t)pthread_self());
#endif
    fprintf(output, "\",\"timestamp_us\":%" PRIu64 ",\"basis\":\"explicit\"", timestamp);
    if (context && strcmp(context->run_id, run_id) == 0 && *context->context_id) {
        fputs(",\"context_id\":", output);
        string(output, context->context_id);
    }
    return 1;
}
static void number(double value) {
#ifdef _WIN32
    static _locale_t numeric_locale;
    if (!numeric_locale)
        numeric_locale = _create_locale(LC_NUMERIC, "C");
    _fprintf_l(output, "%.17g", numeric_locale, value);
#else
    static locale_t numeric_locale;
    if (!numeric_locale)
        numeric_locale = newlocale(LC_NUMERIC_MASK, "C", (locale_t)0);
    locale_t previous = uselocale(numeric_locale);
    fprintf(output, "%.17g", value);
    uselocale(previous);
#endif
}
static void finish(void) {
    fputs(",\"attributes\":{\"clock\":\"unix_epoch\",\"clock_alignment\":\"direct_epoch\",\"clock_"
          "source\":\"epoch clock with monotonic "
          "duration\",\"thread_id_source\":",
          output);
#ifdef _WIN32
    string(output, "GetCurrentThreadId");
#else
    string(output, "pthread_self");
#endif
    if (current_metric)
        fputs(",\"temporality\":\"delta\"", output);
    fputs("}}\n", output);
    if (fflush(output) != 0 || ferror(output))
        failure = errno ? errno : EIO;
}
int hunin_enabled(void) {
    int result;
    LOCK();
    result = configured() && !failure;
    UNLOCK();
    return result;
}
int hunin_last_error(void) {
    int result;
    LOCK();
    result = failure;
    UNLOCK();
    return result;
}
int hunin_flush(void) {
    LOCK();
    if (output) {
        if (fflush(output) != 0)
            failure = errno;
#ifdef _WIN32
        if (_commit(_fileno(output)) != 0)
            failure = errno;
#else
        if (fsync(fileno(output)) != 0)
            failure = errno;
#endif
    }
    int result = failure;
    UNLOCK();
    return result;
}
hunin_context_t hunin_context_current_run(void) {
    hunin_context_t context;
    memset(&context, 0, sizeof(context));
    if (configured())
        copy_id(context.run_id, getenv("HUNIN_RUN_ID"));
    return context;
}
hunin_context_t hunin_context_new(const char *name) {
    hunin_context_t context = hunin_context_current_run();
    LOCK();
    if (begin("context", context.context_id, NULL, epoch_us())) {
        fputs(",\"name\":", output);
        string(output, name);
        finish();
    }
    UNLOCK();
    return context;
}
void hunin_span_begin(hunin_span_t *span, const char *name, const hunin_context_t *parent) {
    if (!span)
        return;
    memset(span, 0, sizeof(*span));
    span->context = hunin_context_current_run();
    if (parent && strcmp(parent->run_id, span->context.run_id) == 0)
        span->context = *parent;
    if (!configured())
        return;
    LOCK();
    if (prepare()) {
        size_t length = strlen(name ? name : "");
        span->name = (char *)malloc(length + 1);
        if (span->name) {
            memcpy(span->name, name ? name : "", length + 1);
            identifier(span->id);
        } else
            failure = ENOMEM;
    }
    UNLOCK();
    span->owner_pid = PID();
    span->timestamp_us = epoch_us();
    span->started_us = monotonic_us();
}
void hunin_span_end(hunin_span_t *span) {
    if (!span || span->ended)
        return;
    span->ended = 1;
    LOCK();
    if (span->owner_pid == PID() && *span->id &&
        begin("span", span->id, &span->context, span->timestamp_us)) {
        fputs(",\"name\":", output);
        string(output, span->name);
        uint64_t elapsed = monotonic_us() - span->started_us;
        fprintf(output, ",\"duration_us\":%" PRIu64, elapsed);
        if (*span->context.parent_id) {
            fputs(",\"parent_id\":", output);
            string(output, span->context.parent_id);
        }
        finish();
    }
    UNLOCK();
    free(span->name);
    span->name = NULL;
}
hunin_context_t hunin_span_context(const hunin_span_t *span) {
    hunin_context_t context = span->context;
    copy_id(context.parent_id, span->id);
    return context;
}
void hunin_counter(const hunin_context_t *context, const char *name, double value,
                   const char *unit) {
    char id[HUNIN_ID_CAPACITY] = {0};
    if (!isfinite(value))
        return;
    LOCK();
    if (begin("metric", id, context, epoch_us())) {
        fputs(",\"name\":", output);
        string(output, name);
        fputs(",\"value\":", output);
        number(value);
        fputs(",\"unit\":", output);
        string(output, unit ? unit : "count");
        fputs(",\"scope\":", output);
        string(output, context && *context->context_id ? "context" : "thread");
        finish();
    }
    UNLOCK();
}
void hunin_event(const hunin_context_t *context, const char *name) {
    char id[HUNIN_ID_CAPACITY] = {0};
    LOCK();
    if (begin("event", id, context, epoch_us())) {
        fputs(",\"name\":", output);
        string(output, name);
        finish();
    }
    UNLOCK();
}
void hunin_relation(const hunin_context_t *context, const char *from, const char *to,
                    const char *relation) {
    char id[HUNIN_ID_CAPACITY] = {0};
    if (!from || !to || !*from || !*to)
        return;
    LOCK();
    if (begin("relation", id, context, epoch_us())) {
        fputs(",\"from\":", output);
        string(output, from);
        fputs(",\"to\":", output);
        string(output, to);
        fputs(",\"relation\":", output);
        string(output, relation ? relation : "links");
        fputs(",\"evidence\":[]", output);
        finish();
    }
    UNLOCK();
}
