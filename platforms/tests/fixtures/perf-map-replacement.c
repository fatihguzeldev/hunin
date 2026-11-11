#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static atomic_int stopped;
static void *idle(void *unused) {
  (void)unused;
  while (!atomic_load(&stopped)) usleep(10000);
  return NULL;
}
static uint64_t now(void) {
  struct timespec value;
  if (clock_gettime(CLOCK_MONOTONIC, &value)) abort();
  return (uint64_t)value.tv_sec * 1000000000 + value.tv_nsec;
}
int main(int argc, char **argv) {
  if (argc != 3) return 1;
  alarm(12);
  setvbuf(stdout, NULL, _IONBF, 0);
  void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (!library) return 2;
  unsigned long (*work)(unsigned long) = dlsym(library, "mapping_a_work");
  if (!work) return 3;
  FILE *maps = fopen("/proc/self/maps", "r");
  if (!maps) return 4;
  unsigned long begin = 0, end = 0, offset = 0;
  char line[4096], permissions[5];
  while (fgets(line, sizeof(line), maps)) {
    if (sscanf(line, "%lx-%lx %4s %lx", &begin, &end, permissions, &offset) == 4 &&
        begin <= (uintptr_t)work && (uintptr_t)work < end) break;
  }
  fclose(maps);
  if (!(begin <= (uintptr_t)work && (uintptr_t)work < end)) return 5;
  pthread_t threads[2];
  for (unsigned n = 0; n < 2; ++n)
    if (pthread_create(&threads[n], NULL, idle, NULL)) return 6;
  printf("ready %lx %lx\n", begin, end);
  if (!fgets(line, sizeof(line), stdin) || line[0] != 's') return 7;
  int replacement = open(argv[2], O_RDONLY);
  if (replacement < 0 || mmap((void *)begin, end - begin, PROT_READ | PROT_EXEC,
                            MAP_PRIVATE | MAP_FIXED, replacement, offset) != (void *)begin) return 8;
  close(replacement);
  printf("switched %llu\n", (unsigned long long)now());
  if (!fgets(line, sizeof(line), stdin) || line[0] != 'g') return 9;
  uint64_t deadline = now() + 700000000;
  volatile unsigned long output = 0;
  while (now() < deadline) output ^= work(1000000);
  atomic_store(&stopped, 1);
  for (unsigned n = 0; n < 2; ++n) pthread_join(threads[n], NULL);
  printf("done %lu\n", output);
  return 0;
}
