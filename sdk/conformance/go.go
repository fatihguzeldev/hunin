package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	h "github.com/fatihguzeldev/hunin/sdk/go"
	"os"
	"strings"
	"sync"
)

func main() {
	if len(os.Args) > 1 && os.Args[1] == "--child" {
		var token h.Propagation
		_ = json.Unmarshal([]byte(os.Getenv("HUNIN_SDK_TOKEN")), &token)
		ctx, remote := h.Start(h.AttachContext(context.Background(), token), "remote", nil)
		h.Counter(ctx, "jobs", 1, "count", nil)
		fmt.Println("HUNIN_REMOTE " + remote.ID)
		remote.End()
	} else {
		ctx, root := h.Start(h.NewContext(context.Background(), "request", nil), "root", nil)
		var workers sync.WaitGroup
		for _, name := range []string{"a", "b"} {
			workers.Add(1)
			go func(name string) {
				defer workers.Done()
				nested, parent := h.Start(ctx, name, nil)
				_, child := h.Start(nested, name+"-child", nil)
				h.Counter(nested, "jobs", 1, "count", nil)
				child.End()
				parent.End()
			}(name)
		}
		workers.Wait()
		token, _ := json.Marshal(h.CaptureContext(ctx))
		fmt.Println("HUNIN_TOKEN " + string(token))
		remote, _ := bufio.NewReader(os.Stdin).ReadString('\n')
		remote = strings.TrimSpace(remote)
		if root.ID != "" && remote != "" {
			h.Relation(ctx, root.ID, remote, "dispatches", nil)
		}
		root.End()
	}
	if err := h.Flush(); err != nil {
		panic(err)
	}
}
