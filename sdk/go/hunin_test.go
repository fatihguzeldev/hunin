package hunin

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

type afterFailureMarshaler struct{ called *bool }

func (value afterFailureMarshaler) MarshalJSON() ([]byte, error) {
	*value.called = true
	return []byte(`"unexpected"`), nil
}

func TestWriteFailureDisablesNewPropagationTokens(t *testing.T) {
	if os.Getenv("HUNIN_GO_FAILURE_PROBE") == "1" {
		type applicationKey struct{}
		application := context.WithValue(context.Background(), applicationKey{}, 42)
		ctx := NewContext(application, "undeliverable", nil)
		failure := LastError()
		if failure == nil || Flush() == nil || Enabled() {
			t.Fatal("write failure was not sticky")
		}
		if token := CaptureContext(ctx); token != (Propagation{}) {
			t.Fatalf("failed producer still propagates context: %+v", token)
		}
		child, span := Start(ctx, "after-error", nil)
		if span.ID != "" || CaptureContext(child) != (Propagation{}) {
			t.Fatal("failed producer allocated a new span identity")
		}
		if child.Value(applicationKey{}) != 42 {
			t.Fatal("disabled instrumentation changed application context")
		}
		called := false
		if Event(ctx, "after-error", Attributes{"value": afterFailureMarshaler{&called}}) != "" || called {
			t.Fatal("failed producer continued user attribute serialization")
		}
		span.End()
		if LastError() != failure {
			t.Fatal("later operations replaced the original write failure")
		}
		return
	}
	blocked := filepath.Join(t.TempDir(), "not-a-directory")
	if err := os.WriteFile(blocked, []byte("blocked-sdk-output"), 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestWriteFailureDisablesNewPropagationTokens$")
	command.Env = append(os.Environ(), "HUNIN_RUN_ID=write-error", "HUNIN_RECORD_DIR="+blocked, "HUNIN_GO_FAILURE_PROBE=1")
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("isolated SDK failure probe: %v\n%s", err, output)
	}
}

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
