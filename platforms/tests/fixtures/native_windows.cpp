#include <chrono>
#include <cstdint>
#include <cstdio>
volatile std::uint64_t result;
__declspec(noinline) std::uint64_t prework_probe(std::uint64_t n) {
    const auto start = std::chrono::steady_clock::now();
    while (std::chrono::steady_clock::now()-start<std::chrono::milliseconds(200))
        n = n*6364136223846793005ULL+1442695040888963407ULL;
    return n;
}
__declspec(noinline) std::uint64_t render_block(std::uint64_t n) {
    for (std::uint64_t i=0; i<1000000; ++i) n=(n*6364136223846793005ULL+1442695040888963407ULL)^i;
    return n;
}
int main() {
    std::puts("ready");std::fflush(stdout);if(std::getchar()!='p')return 2;
    result=prework_probe(1);std::puts("probe-complete");std::fflush(stdout);
    if(std::getchar()!='g')return 3;
    const auto start=std::chrono::steady_clock::now();std::uint64_t n=1;
    while(std::chrono::steady_clock::now()-start<std::chrono::seconds(3))n=render_block(n);
    result=n;std::printf("%llu\n",(unsigned long long)n);
}
