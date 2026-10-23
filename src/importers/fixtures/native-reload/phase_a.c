#include <time.h>
__attribute__((noinline)) double phase_hot(void) {
 struct timespec start, now; clock_gettime(CLOCK_MONOTONIC,&start); volatile double sum=0;
 do {for(int i=1;i<100000;i++) sum+=i*0.1234567; clock_gettime(CLOCK_MONOTONIC,&now);} while ((now.tv_sec-start.tv_sec)*1000000000LL+now.tv_nsec-start.tv_nsec<400000000LL);
 return sum;
}
