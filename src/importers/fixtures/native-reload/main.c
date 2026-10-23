#include <dlfcn.h>
#include <stdio.h>
#include <unistd.h>
#include <time.h>
int main(void) {
 const char *paths[]={"./phase_a.dylib","./phase_b.dylib","./phase_a.dylib"};
 for(int i=0;i<3;i++) {void *lib=dlopen(paths[i],RTLD_NOW); if(!lib){fputs(dlerror(),stderr);return 1;} double (*fn)(void)=dlsym(lib,"phase_hot"); printf("load %d %s %p\n",i,paths[i],(void*)fn); fflush(stdout); printf("result %f\n",fn()); dlclose(lib); printf("unload %d\n",i);fflush(stdout); usleep(50000);}
 return 0;
}
