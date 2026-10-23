/* Controlled prerequisite-compatible collector that fails before launch. */
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
  if (argc > 1 && !strcmp(argv[1], "--version")) {
    puts("samply 0.13.1-hunin-native1");
    return 0;
  }
  if (argc > 2 && !strcmp(argv[1], "record") && !strcmp(argv[2], "--help")) {
    puts("--lifetime-output --unstable-presymbolicate");
    return 0;
  }
  fputs("controlled collector failure before target launch\n", stderr);
  return 73;
}
