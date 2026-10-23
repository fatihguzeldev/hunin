// Windows-only controller fixture: stops only the exact session it created.
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <evntrace.h>
#include <stdio.h>
#include <stdint.h>
#include <stddef.h>
#include <wchar.h>
#include <stdlib.h>

typedef struct {
    EVENT_TRACE_PROPERTIES p;
    wchar_t name[1024];
    wchar_t file[32768];
} properties;
static const GUID kernel_guid = {0x9e814aad,0x3204,0x11d2,{0x9a,0x82,0x00,0x60,0x08,0xa8,0x69,0x39}};
static properties *make_properties(void) {
    properties *p = calloc(1,sizeof(*p));
    if (!p) exit(90);
    p->p.Wnode.BufferSize=sizeof(*p);
    p->p.LoggerNameOffset=offsetof(properties,name);
    p->p.LogFileNameOffset=offsetof(properties,file);
    return p;
}
static ULONG query(properties *p) {
    return ControlTraceW(0,KERNEL_LOGGER_NAMEW,&p->p,EVENT_TRACE_CONTROL_QUERY);
}
static const wchar_t *returned_file(const properties *p) {
    size_t offset=p->p.LogFileNameOffset;
    if(offset<sizeof(EVENT_TRACE_PROPERTIES) || offset>=sizeof(*p) || offset%sizeof(wchar_t))return NULL;
    const wchar_t *file=(const wchar_t *)((const unsigned char *)p+offset);
    size_t capacity=(sizeof(*p)-offset)/sizeof(wchar_t);
    return wmemchr(file,0,capacity)?file:NULL;
}
static BOOL WINAPI ignore_break(DWORD event) { return event == CTRL_BREAK_EVENT; }
int wmain(int argc,wchar_t **argv) {
    if (argc >= 3 && !wcscmp(argv[1],L"work")) {
        SetConsoleCtrlHandler(ignore_break,TRUE);
        FILE *marker=_wfopen(argv[2],L"wb"); if(!marker)return 21;
        fputs("ready",marker); fclose(marker);
        volatile uint64_t value=1;
        DWORD started=GetTickCount();
        for (;;) {
            for(int i=0;i<100000;i++)value=value*1664525u+1013904223u;
            // Only the direct-wrapper-death control has a bounded self-release.
            // The Engine cancellation control has no release path and must force.
            if(argc>=4 && (GetFileAttributesW(argv[3])!=INVALID_FILE_ATTRIBUTES || GetTickCount()-started>20000))return 0;
        }
    }
    if(argc==4 && !wcscmp(argv[1],L"kill-wrapper")) {
        DWORD pid=wcstoul(argv[2],NULL,10);
        uint64_t expected=_wcstoui64(argv[3],NULL,10);
        HANDLE process=OpenProcess(PROCESS_TERMINATE|PROCESS_QUERY_LIMITED_INFORMATION|SYNCHRONIZE,FALSE,pid);
        if(!process)return 31;
        FILETIME created,exited,kernel,user;
        if(!GetProcessTimes(process,&created,&exited,&kernel,&user) || (((uint64_t)created.dwHighDateTime<<32)|created.dwLowDateTime)!=expected) {
            CloseHandle(process);fputs("Wrapper birth changed; refusing termination\n",stderr);return 32;
        }
        if(!TerminateProcess(process,42)){CloseHandle(process);return 33;}
        DWORD result=WaitForSingleObject(process,3000);CloseHandle(process);
        return result==WAIT_OBJECT_0?0:34;
    }
    properties *p=make_properties();
    ULONG status=query(p);
    if(argc==2 && !wcscmp(argv[1],L"query")) {
        if(status==ERROR_WMI_INSTANCE_NOT_FOUND) {puts("absent");return 0;}
        if(status!=ERROR_SUCCESS) {fprintf(stderr,"query Windows error%lu\n",status);return 2;}
        printf("%llx\n",(unsigned long long)p->p.Wnode.HistoricalContext);return 0;
    }
    if(argc==3 && !wcscmp(argv[1],L"start")) {
        if(status!=ERROR_WMI_INSTANCE_NOT_FOUND) {fputs("Existing kernel logger; fixture will not mutate it\n",stderr);return 3;}
        ZeroMemory(p,sizeof(*p));
        p->p.Wnode.BufferSize=sizeof(*p);
        p->p.LoggerNameOffset=offsetof(properties,name);
        p->p.LogFileNameOffset=offsetof(properties,file);
        p->p.Wnode.Guid=kernel_guid;
        p->p.Wnode.ClientContext=1;
        p->p.Wnode.Flags=WNODE_FLAG_TRACED_GUID;
        p->p.LogFileMode=EVENT_TRACE_FILE_MODE_SEQUENTIAL;
        p->p.EnableFlags=EVENT_TRACE_FLAG_PROCESS;
        wcscpy_s(p->file,32768,argv[2]);
        TRACEHANDLE handle=0;
        status=StartTraceW(&handle,KERNEL_LOGGER_NAMEW,&p->p);
        if(status) {fprintf(stderr,"start Windows error%lu\n",status);return 4;}
        printf("%llx\n",(unsigned long long)handle);return 0;
    }
    if(argc==4 && !wcscmp(argv[1],L"stop")) {
        TRACEHANDLE expected=_wcstoui64(argv[2],NULL,16);
        if(status==ERROR_WMI_INSTANCE_NOT_FOUND) {puts("absent");return 0;}
        const wchar_t *file=returned_file(p);
        if(status!=ERROR_SUCCESS || p->p.Wnode.HistoricalContext!=expected || !file || _wcsicmp(file,argv[3])) {
            fputs("Controller identity/path changed; fixture refuses stop\n",stderr);return 5;
        }
        ZeroMemory(p,sizeof(*p));p->p.Wnode.BufferSize=sizeof(*p);
        status=ControlTraceW(expected,NULL,&p->p,EVENT_TRACE_CONTROL_STOP);
        if(status!=ERROR_SUCCESS && status!=ERROR_MORE_DATA && status!=ERROR_ACTIVE_CONNECTIONS && status!=ERROR_WMI_INSTANCE_NOT_FOUND) {
            fprintf(stderr,"stop Windows error%lu\n",status);return 6;
        }
        for(int i=0;i<150;i++) {
            free(p);p=make_properties();status=query(p);
            if(status==ERROR_WMI_INSTANCE_NOT_FOUND || (status==ERROR_SUCCESS && p->p.Wnode.HistoricalContext!=expected)) {puts("absent");return 0;}
            Sleep(20);
        }
        fputs("Owned control session still stopping\n",stderr);return 7;
    }
    fputs("usage: query | start <etl> | stop <handle> <etl> | work <marker>\n",stderr);return 9;
}
