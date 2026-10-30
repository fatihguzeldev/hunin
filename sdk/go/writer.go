// Process-owned JSONL producer, file serialization and sticky failure state.
package hunin

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"
)

var writer struct {
	sync.Mutex
	file    *os.File
	failure error
}
var runID = os.Getenv("HUNIN_RUN_ID")
var directory = os.Getenv("HUNIN_RECORD_DIR")
var producerID = identifier()

func identifier() string {
	var value [16]byte
	if _, err := rand.Read(value[:]); err == nil {
		return hex.EncodeToString(value[:])
	}
	return strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 16)
}
func Enabled() bool {
	writer.Lock()
	defer writer.Unlock()
	return runID != "" && directory != "" && writer.failure == nil
}
func LastError() error { writer.Lock(); defer writer.Unlock(); return writer.failure }
func emit(ctx context.Context, kind string, fields map[string]any) string {
	if !Enabled() {
		return ""
	}
	token := CaptureContext(ctx)
	record := map[string]any{"schema_version": 1, "kind": kind, "id": identifier(), "run_id": runID, "producer_id": producerID, "pid": os.Getpid(), "tid": "", "timestamp_us": time.Now().UnixMicro(), "basis": "explicit"}
	if token.ContextID != "" {
		record["context_id"] = token.ContextID
	}
	for key, value := range fields {
		record[key] = value
	}
	metadata := Attributes{"thread_id_source": "unavailable; goroutines may migrate", "clock": "time.Time Unix epoch with monotonic duration"}
	if value, ok := record["attributes"].(Attributes); ok {
		for key, item := range value {
			metadata[key] = item
		}
	}
	record["attributes"] = metadata
	payload, err := json.Marshal(record)
	writer.Lock()
	defer writer.Unlock()
	if writer.failure != nil {
		return ""
	}
	if err == nil && writer.file == nil {
		err = os.MkdirAll(directory, 0700)
		if err == nil {
			writer.file, err = os.OpenFile(filepath.Join(directory, "sdk-go-"+strconv.Itoa(os.Getpid())+"-"+producerID+".jsonl"), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0600)
		}
	}
	if err == nil {
		payload = append(payload, '\n')
		var written int
		written, err = writer.file.Write(payload)
		if err == nil && written != len(payload) {
			err = os.ErrInvalid
		}
	}
	if err != nil {
		writer.failure = err
		return ""
	}
	return record["id"].(string)
}

// Flush makes all prior observations durable. Hunin never starts a background goroutine.
func Flush() error {
	writer.Lock()
	defer writer.Unlock()
	if writer.file != nil {
		if err := writer.file.Sync(); err != nil {
			writer.failure = err
		}
	}
	return writer.failure
}
