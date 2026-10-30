// Package hunin records optional explicit observations when launched by Hunin.
package hunin

import (
	"context"
	"math"
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
