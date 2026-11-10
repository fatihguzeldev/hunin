#include <chrono>
#include <cstdint>
#include <cstdio>
#include <sys/resource.h>
volatile std::uint64_t result;
__attribute__((noinline)) std::uint64_t render_block(std::uint64_t n) {
    for (std::uint64_t i=0; i<1000000; ++i) n=(n*6364136223846793005ULL+1442695040888963407ULL)^i;
    return n;
}
int main() {
    std::puts("ready"); std::fflush(stdout); if(std::getchar()!='g') return 2;
    auto start=std::chrono::steady_clock::now(); std::uint64_t count=0,n=1;
    while(std::chrono::steady_clock::now()-start<std::chrono::seconds(3)){n=render_block(n);++count;}
    result=n; rusage usage{}; getrusage(RUSAGE_SELF,&usage);
    std::printf("%llu %llu %lld\n",(unsigned long long)count,(unsigned long long)n,(long long)(usage.ru_utime.tv_sec*1000000LL+usage.ru_utime.tv_usec+usage.ru_stime.tv_sec*1000000LL+usage.ru_stime.tv_usec));
}
