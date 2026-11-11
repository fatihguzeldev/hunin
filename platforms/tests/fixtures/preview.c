#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#ifdef _WIN32
#include <windows.h>
#define NOINLINE __declspec(noinline)
#define SEPARATOR "\\"
#else
#include <dirent.h>
#define NOINLINE __attribute__((noinline))
#define SEPARATOR "/"
#endif
#ifdef __cplusplus
#define LINKAGE extern "C"
#else
#define LINKAGE
#endif
#ifdef HUNIN_COVERAGE
LINKAGE int __llvm_profile_write_file(void);
static void snapshot(void){if(__llvm_profile_write_file()!=0)_Exit(31);}
#else
static void snapshot(void){}
#endif
static uint64_t ticks(void){
#ifdef _WIN32
    LARGE_INTEGER value;QueryPerformanceCounter(&value);return (uint64_t)value.QuadPart;
#else
    struct timespec value;clock_gettime(CLOCK_MONOTONIC,&value);return (uint64_t)value.tv_sec*1000000000ULL+(uint64_t)value.tv_nsec;
#endif
}
static uint64_t frequency(void){
#ifdef _WIN32
    LARGE_INTEGER value;QueryPerformanceFrequency(&value);return (uint64_t)value.QuadPart;
#else
    return 1000000000ULL;
#endif
}
static volatile uint64_t sink;
LINKAGE NOINLINE uint64_t resize_preview(const unsigned char *input, unsigned width, unsigned height, volatile unsigned char *output){
    uint64_t checksum=0;
    for(unsigned y=0;y<32;y++)for(unsigned x=0;x<32;x++)for(unsigned c=0;c<3;c++){
        unsigned char value=input[((size_t)(y*height/32)*width+x*width/32)*3+c];
        output[(y*32+x)*3+c]=value;checksum+=value;
    }
    return checksum;
}
static int compare_names(const void*a,const void*b){return strcmp((const char*)a,(const char*)b);}
static int names(const char*directory,char result[256][260]){
    unsigned count=0;
#ifdef _WIN32
    char pattern[4096];if(snprintf(pattern,sizeof(pattern),"%s" SEPARATOR "*.ppm",directory)<0)return -1;
    WIN32_FIND_DATAA data;HANDLE handle=FindFirstFileA(pattern,&data);if(handle==INVALID_HANDLE_VALUE)return -1;
    do{if(!(data.dwFileAttributes&FILE_ATTRIBUTE_DIRECTORY)){if(count==256){FindClose(handle);return -1;}strcpy(result[count++],data.cFileName);}}while(FindNextFileA(handle,&data));FindClose(handle);
#else
    DIR*dir=opendir(directory);if(!dir)return -1;struct dirent*entry;
    while((entry=readdir(dir))!=NULL){size_t length=strlen(entry->d_name);if(length>4&&length<260&&strcmp(entry->d_name+length-4,".ppm")==0){if(count==256){closedir(dir);return -1;}strcpy(result[count++],entry->d_name);}}closedir(dir);
#endif
    qsort(result,count,260,compare_names);return (int)count;
}
int main(int argc,char**argv){
    if(argc!=4)return 20;const char*input_dir=argv[1],*output_dir=argv[2];unsigned passes=strcmp(argv[3],"redundant")==0?2:1;
    const char*setting=getenv("PREVIEW_REPETITIONS");unsigned long repetitions=setting?strtoul(setting,NULL,10):10000;if(!repetitions||repetitions>1000000)return 21;
    char files[256][260];int count=names(input_dir,files);if(count<=0)return 22;
    snapshot();puts("HUNIN_READY");fflush(stdout);char gate[128];if(!fgets(gate,sizeof(gate),stdin)||strcmp(gate,"HUNIN_GO\n")!=0)_Exit(0);
    uint64_t work_start=ticks();
    char path[4096];snprintf(path,sizeof(path),"%s" SEPARATOR "manifest.json",output_dir);FILE*manifest=fopen(path,"wb");if(!manifest)return 23;
    fputs("{\"algorithm\":\"nearest-neighbor\",\"width\":32,\"height\":32,\"assets\":[",manifest);
    for(int asset=0;asset<count;asset++){
        snprintf(path,sizeof(path),"%s" SEPARATOR "%s",input_dir,files[asset]);FILE*input=fopen(path,"rb");if(!input)return 24;unsigned w,h,max;
        if(fscanf(input,"P6 %u %u %u%*c",&w,&h,&max)!=3||max!=255||!w||!h||w>4096||h>4096)return 25;
        size_t length=(size_t)w*h*3;unsigned char*bytes=(unsigned char*)malloc(length);if(!bytes||fread(bytes,1,length,input)!=length||fgetc(input)!=EOF)return 26;fclose(input);
        unsigned char preview[3072];uint64_t checksum=0;for(unsigned pass=0;pass<passes;pass++)for(unsigned long repeat=0;repeat<repetitions;repeat++){checksum=resize_preview(bytes,w,h,preview);sink+=checksum;}
        char name[280];size_t stem=strlen(files[asset])-4;snprintf(name,sizeof(name),"%.*s.preview.ppm",(int)stem,files[asset]);snprintf(path,sizeof(path),"%s" SEPARATOR "%s",output_dir,name);FILE*output=fopen(path,"wb");if(!output)return 27;fputs("P6\n32 32\n255\n",output);if(fwrite(preview,1,sizeof(preview),output)!=sizeof(preview)||fclose(output)!=0)return 28;
        if(asset)fputc(',',manifest);fprintf(manifest,"{\"input\":\"%s\",\"output\":\"%s\",\"source_width\":%u,\"source_height\":%u,\"bytes\":3085,\"checksum\":%llu}",files[asset],name,w,h,(unsigned long long)checksum);free(bytes);
    }
    if(fputs("]}",manifest)<0||fclose(manifest)!=0)return 29;
    uint64_t work_end=ticks();const char*metrics_path=getenv("HUNIN_WORK_METRICS");if(metrics_path){FILE*metrics=fopen(metrics_path,"wb");if(!metrics)return 30;fprintf(metrics,"{\"start\":%llu,\"end\":%llu,\"ticks_per_second\":%llu,\"scope\":\"asset decode/resize/output batch excluding profile snapshots; input names prepared before ready\"}",(unsigned long long)work_start,(unsigned long long)work_end,(unsigned long long)frequency());if(fclose(metrics)!=0)return 32;}
    snapshot();puts("HUNIN_WORK_COMPLETE");fflush(stdout);if(!fgets(gate,sizeof(gate),stdin)||strcmp(gate,"HUNIN_EXIT\n")!=0)_Exit(0);_Exit(0);
}
