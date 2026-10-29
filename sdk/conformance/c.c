#include "hunin.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static void field(char *target, const char *name) {
    const char *value = getenv(name);
    snprintf(target, HUNIN_ID_CAPACITY, "%s", value ? value : "");
}
int main(int argc, char **argv) {
    hunin_context_t context;
    hunin_span_t root;
    if (argc > 1 && strcmp(argv[1], "--child") == 0) {
        memset(&context, 0, sizeof(context));
        field(context.run_id, "HUNIN_SDK_PARENT_RUN");
        field(context.context_id, "HUNIN_SDK_PARENT_CONTEXT");
        field(context.parent_id, "HUNIN_SDK_PARENT_ID");
        hunin_span_begin(&root, "remote", &context);
        hunin_counter(&context, "jobs", 1, "count");
        printf("HUNIN_REMOTE %s\n", root.id);
        hunin_span_end(&root);
    } else {
        context = hunin_context_new("request");
        hunin_span_begin(&root, "root", &context);
        hunin_context_t token = hunin_span_context(&root);
        const char *names[] = {"a", "b"};
        const char *children[] = {"a-child", "b-child"};
        int i;
        for (i = 0; i < 2; i++) {
            hunin_span_t parent, child;
            hunin_span_begin(&parent, names[i], &token);
            hunin_context_t nested = hunin_span_context(&parent);
            hunin_span_begin(&child, children[i], &nested);
            hunin_counter(&nested, "jobs", 1, "count");
            hunin_span_end(&child);
            hunin_span_end(&parent);
        }
        printf("HUNIN_TOKEN "
               "{\"run_id\":\"%s\",\"context_id\":\"%s\",\"parent_id\":\"%s\"}\n",
               token.run_id, token.context_id, token.parent_id);
        fflush(stdout);
        char remote[HUNIN_ID_CAPACITY];
        if (fgets(remote, sizeof(remote), stdin)) {
            remote[strcspn(remote, "\r\n")] = 0;
            if (*root.id && *remote)
                hunin_relation(&token, root.id, remote, "dispatches");
        }
        hunin_span_end(&root);
    }
    return hunin_flush();
}
