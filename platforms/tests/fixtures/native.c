#include <stdint.h>
#include <stdio.h>
#include <time.h>
volatile uint64_t result;
__attribute__((noinline)) uint64_t render_block(uint64_t n) {
    for (uint64_t i=0; i<1000000; ++i) n=(n*6364136223846793005ULL+1442695040888963407ULL)^i;
    return n;
}
int main(void) {
    puts("ready"); fflush(stdout); if(getchar()!='g') return 2;
    struct timespec start,now; clock_gettime(CLOCK_MONOTONIC,&start); uint64_t n=1;
    do { n=render_block(n); clock_gettime(CLOCK_MONOTONIC,&now); }
    while((now.tv_sec-start.tv_sec)*1000000000LL+now.tv_nsec-start.tv_nsec<3000000000LL);
    result=n;printf("%llu\n",(unsigned long long)n);return 0;
}
