#ifndef HUNIN_HPP
#define HUNIN_HPP
#include "hunin.h"
namespace hunin {
using Context = hunin_context_t;
inline Context context(const char *name) { return hunin_context_new(name); }
class Span {
    hunin_span_t value_{};

  public:
    explicit Span(const char *name, const Context *parent = nullptr) {
        hunin_span_begin(&value_, name, parent);
    }
    ~Span() { hunin_span_end(&value_); }
    Span(const Span &) = delete;
    Span &operator=(const Span &) = delete;
    Context context() const { return hunin_span_context(&value_); }
    const char *id() const { return value_.id; }
    void end() { hunin_span_end(&value_); }
};
inline void counter(const Context &context, const char *name, double value,
                    const char *unit = "count") {
    hunin_counter(&context, name, value, unit);
}
inline void event(const Context &context, const char *name) { hunin_event(&context, name); }
inline void relation(const Context &context, const char *from, const char *to,
                     const char *name = "links") {
    hunin_relation(&context, from, to, name);
}
inline int flush() { return hunin_flush(); }
} // namespace hunin
#endif
