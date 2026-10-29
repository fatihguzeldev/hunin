#include <chrono>
#include <cstdint>
#include <cstdio>
#include <thread>
#ifdef _WIN32
#include <windows.h>
using Library=HMODULE;
static Library load(const char *path) { return LoadLibraryA(path); }
static void close(Library library) { FreeLibrary(library); }
static void *symbol(Library library) { return reinterpret_cast<void *>(GetProcAddress(library,"phase_hot")); }
#else
#include <dlfcn.h>
using Library=void *;
static Library load(const char *path) { return dlopen(path,RTLD_NOW|RTLD_LOCAL); }
static void close(Library library) { dlclose(library); }
static void *symbol(Library library) { return dlsym(library,"phase_hot"); }
#endif
using Hot=std::uint64_t(*)(int);
static void busy(int milliseconds) {
  const auto end=std::chrono::steady_clock::now()+std::chrono::milliseconds(milliseconds);
  volatile std::uint64_t value=1;
  do {for(int i=0;i<1024;i++)value=value*17+1;}while(std::chrono::steady_clock::now()<end);
}
static void marker(const char *kind,int phase,void *address=nullptr) {
  const auto epoch=std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::system_clock::now().time_since_epoch()).count();
  std::printf("{\"event\":\"%s\",\"phase\":%d,\"epoch_us\":%lld,\"address\":\"%p\"}\n",kind,phase,static_cast<long long>(epoch),address);
  std::fflush(stdout);
}
int main(int argc,char **argv) {
  if(argc!=4)return 10;
  for(int phase=0;phase<3;phase++) {
    marker("before_load",phase);
    Library library=load(argv[phase==1?2:1]);if(!library)return 11;
    Hot hot=reinterpret_cast<Hot>(symbol(library));if(!hot)return 12;
    marker("loaded",phase,reinterpret_cast<void *>(hot));hot(220);
    marker("before_unload",phase);close(library);marker("unloaded",phase);busy(100);
  }
  Library first=load(argv[1]),alias=load(argv[3]);if(!first||!alias)return 13;
  Hot a=reinterpret_cast<Hot>(symbol(first)),b=reinterpret_cast<Hot>(symbol(alias));if(!a||!b)return 14;
  marker("simultaneous",3,reinterpret_cast<void *>(a));marker("simultaneous",4,reinterpret_cast<void *>(b));
  if(a==b)return 15;
  std::thread worker([a]{a(180);});b(180);worker.join();close(first);close(alias);busy(100);
  for(int i=0;i<2;i++){std::thread short_lived([]{busy(60);});short_lived.join();busy(30);}
  marker("finished",5);return 0;
}
