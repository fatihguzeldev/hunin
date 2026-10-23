package hunin

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

func TestConcurrentContextsAndFlush(t *testing.T) {
	directory = t.TempDir()
	runID = "test"
	producerID = identifier()
	defer func() {
		writer.Lock()
		if writer.file != nil {
			writer.file.Close()
		}
		writer.file = nil
		writer.failure = nil
		writer.Unlock()
		runID = ""
		directory = ""
	}()
	var workers sync.WaitGroup
	for _, name := range []string{"a", "b"} {
		workers.Add(1)
		go func(name string) {
			defer workers.Done()
			ctx, parent := Start(NewContext(context.Background(), name, nil), name, nil)
			_, child := Start(ctx, name+"-child", nil)
			Counter(ctx, "items", 1, "count", nil)
			child.End()
			parent.End()
		}(name)
	}
	workers.Wait()
	if err := Flush(); err != nil {
		t.Fatal(err)
	}
	files, _ := filepath.Glob(filepath.Join(directory, "*.jsonl"))
	payload, err := os.ReadFile(files[0])
	if err != nil {
		t.Fatal(err)
	}
	spans := map[string]map[string]any{}
	for _, line := range bytes.Split(bytes.TrimSpace(payload), []byte{'\n'}) {
		var row map[string]any
		if err = json.Unmarshal(line, &row); err != nil {
			t.Fatal(err)
		}
		if row["kind"] == "span" {
			spans[row["name"].(string)] = row
		}
	}
	if len(spans) != 4 {
		t.Fatalf("expected4 spans, got%d", len(spans))
	}
	for _, name := range []string{"a", "b"} {
		if spans[name+"-child"]["parent_id"] != spans[name]["id"] {
			t.Fatal("parent crosses contexts")
		}
	}
}
