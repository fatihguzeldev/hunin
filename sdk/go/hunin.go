// Package hunin records optional explicit observations when launched by Hunin.
package hunin

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"
)

type contextKey struct{}

// Propagation is safe to serialize and carry through a job or RPC boundary.
type Propagation struct {
	RunID     string `json:"run_id"`
	ContextID string `json:"context_id,omitempty"`
	ParentID  string `json:"parent_id,omitempty"`
}
type Attributes map[string]any

func attributesOrEmpty(value Attributes) Attributes {
	if value == nil {
		return Attributes{}
	}
	copy := Attributes{}
	for key, item := range value {
		copy[key] = item
	}
	return copy
}

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
	metadata := Attributes{"thread_id_source": "unavailable; goroutines may migrate", "clock": "unix_epoch", "clock_alignment": "direct_epoch", "clock_source": "time.Time Unix epoch with monotonic duration"}
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
func CaptureContext(ctx context.Context) Propagation {
	if !Enabled() {
		return Propagation{}
	}
	if value, ok := ctx.Value(contextKey{}).(Propagation); ok {
		return value
	}
	return Propagation{RunID: runID}
}
func AttachContext(ctx context.Context, token Propagation) context.Context {
	if !Enabled() || token.RunID != runID {
		return ctx
	}
	return context.WithValue(ctx, contextKey{}, token)
}
func NewContext(ctx context.Context, name string, attributes Attributes) context.Context {
	id := emit(ctx, "context", map[string]any{"name": name, "attributes": attributesOrEmpty(attributes)})
	return AttachContext(ctx, Propagation{RunID: runID, ContextID: id})
}

type Span struct {
	ID         string
	ctx        context.Context
	name       string
	attributes Attributes
	started    time.Time
	once       sync.Once
}

// Start returns a child context explicitly; it remains correct when goroutines migrate.
func Start(ctx context.Context, name string, attributes Attributes) (context.Context, *Span) {
	span := &Span{ctx: ctx, name: name, attributes: attributesOrEmpty(attributes), started: time.Now()}
	if Enabled() {
		span.ID = identifier()
	}
	token := CaptureContext(ctx)
	token.ParentID = span.ID
	return AttachContext(ctx, token), span
}
func (span *Span) End() {
	span.once.Do(func() {
		if span.ID == "" {
			return
		}
		fields := map[string]any{"id": span.ID, "name": span.name, "timestamp_us": span.started.UnixMicro(), "duration_us": max(0, time.Since(span.started).Microseconds()), "attributes": attributesOrEmpty(span.attributes)}
		token := CaptureContext(span.ctx)
		if token.ParentID != "" {
			fields["parent_id"] = token.ParentID
		}
		emit(span.ctx, "span", fields)
	})
}
func Counter(ctx context.Context, name string, value float64, unit string, attributes Attributes) string {
	if math.IsNaN(value) || math.IsInf(value, 0) {
		return ""
	}
	if unit == "" {
		unit = "count"
	}
	metadata := Attributes{"temporality": "delta"}
	for key, value := range attributes {
		metadata[key] = value
	}
	scope := "process"
	if CaptureContext(ctx).ContextID != "" {
		scope = "context"
	}
	return emit(ctx, "metric", map[string]any{"name": name, "value": value, "unit": unit, "scope": scope, "attributes": metadata})
}
func Event(ctx context.Context, name string, attributes Attributes) string {
	return emit(ctx, "event", map[string]any{"name": name, "attributes": attributesOrEmpty(attributes)})
}
func Relation(ctx context.Context, from, to, name string, evidence []string) string {
	if from == "" || to == "" {
		return ""
	}
	if name == "" {
		name = "links"
	}
	if evidence == nil {
		evidence = []string{}
	}
	seen := map[string]bool{}
	unique := []string{}
	for _, id := range evidence {
		if !seen[id] {
			seen[id] = true
			unique = append(unique, id)
		}
	}
	return emit(ctx, "relation", map[string]any{"from": from, "to": to, "relation": name, "evidence": unique})
}
