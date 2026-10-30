#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <stdint.h>

#define MAX_SAFE_COUNT UINT64_C(9007199254740991)

typedef struct {
    PyObject *code;
    uint64_t starts;
    uint64_t resumes;
} Entry;

typedef struct {
    Entry *entries;
    size_t slots;
    size_t maximum;
    size_t used;
    uint64_t dropped;
    int overflow;
    PyObject *selectors;
} State;

static int matches(State *state, PyCodeObject *code) {
    if (PyList_GET_SIZE(state->selectors) == 0)
        return 1;
    for (Py_ssize_t i = 0; i < PyList_GET_SIZE(state->selectors); ++i) {
        PyObject *selector = PyList_GET_ITEM(state->selectors, i);
        PyObject *name = PyDict_GetItemString(selector, "name");
        if (PyObject_RichCompareBool(name, code->co_name, Py_EQ) != 1 &&
            PyObject_RichCompareBool(name, code->co_qualname, Py_EQ) != 1)
            continue;
        PyObject *file = PyDict_GetItemString(selector, "file");
        if (file && file != Py_None &&
            PyObject_RichCompareBool(file, code->co_filename, Py_EQ) != 1)
            continue;
        PyObject *line = PyDict_GetItemString(selector, "line");
        if (line && line != Py_None && PyLong_AsLong(line) != code->co_firstlineno)
            continue;
        return 1;
    }
    return 0;
}

static Entry *entry_for(State *state, PyObject *object) {
    if (!state->entries || !PyCode_Check(object))
        return NULL;
    PyCodeObject *code = (PyCodeObject *)object;
    if (!matches(state, code))
        return NULL;
    if (PyUnicode_GET_LENGTH(code->co_filename) > 4096 ||
        PyUnicode_GET_LENGTH(code->co_qualname) > 1024) {
        if (state->dropped < MAX_SAFE_COUNT)
            ++state->dropped;
        return NULL;
    }
    size_t initial = ((uintptr_t)object >> 4) & (state->slots - 1);
    for (size_t distance = 0; distance < state->slots; ++distance) {
        Entry *entry = &state->entries[(initial + distance) & (state->slots - 1)];
        if (entry->code == object)
            return entry;
        if (!entry->code) {
            if (state->used >= state->maximum) {
                if (state->dropped < MAX_SAFE_COUNT)
                    ++state->dropped;
                return NULL;
            }
            Py_INCREF(object);
            entry->code = object;
            ++state->used;
            return entry;
        }
    }
    return NULL;
}

static PyObject *increment(PyObject *self, PyObject *const *args, Py_ssize_t nargs, int resume) {
    if (nargs != 2)
        return PyErr_Format(PyExc_TypeError, "expected code and instruction offset");
    State *state = PyModule_GetState(self);
    Entry *entry = entry_for(state, args[0]);
    if (PyErr_Occurred())
        return NULL;
    if (entry) {
        uint64_t *value = resume ? &entry->resumes : &entry->starts;
        if (*value == MAX_SAFE_COUNT)
            state->overflow = 1;
        else
            ++*value;
    }
    Py_RETURN_NONE;
}

static PyObject *on_start(PyObject *self, PyObject *const *args, Py_ssize_t nargs) {
    return increment(self, args, nargs, 0);
}
static PyObject *on_resume(PyObject *self, PyObject *const *args, Py_ssize_t nargs) {
    return increment(self, args, nargs, 1);
}

static PyObject *configure(PyObject *self, PyObject *args) {
    unsigned int maximum;
    PyObject *selectors;
    if (!PyArg_ParseTuple(args, "IO!", &maximum, &PyList_Type, &selectors))
        return NULL;
    State *state = PyModule_GetState(self);
    if (state->entries || maximum == 0 || maximum > 8192)
        return PyErr_Format(PyExc_ValueError, "configure once with 1..8192 functions");
    if (PyInterpreterState_GetID(PyThreadState_Get()->interp) != 0)
        return PyErr_Format(PyExc_RuntimeError, "subinterpreters are not supported");
    for (Py_ssize_t i = 0; i < PyList_GET_SIZE(selectors); ++i) {
        PyObject *selector = PyList_GET_ITEM(selectors, i);
        PyObject *name = PyDict_Check(selector) ? PyDict_GetItemString(selector, "name") : NULL;
        if (!name || !PyUnicode_Check(name))
            return PyErr_Format(PyExc_ValueError, "selector name must be a string");
    }
    state->maximum = maximum;
    state->slots = 1;
    while (state->slots < (size_t)maximum * 2)
        state->slots *= 2;
    state->entries = PyMem_Calloc(state->slots, sizeof(Entry));
    if (!state->entries)
        return PyErr_NoMemory();
    Py_INCREF(selectors);
    state->selectors = selectors;
    Py_RETURN_NONE;
}

static PyObject *snapshot(PyObject *self, PyObject *unused) {
    (void)unused;
    State *state = PyModule_GetState(self);
    PyObject *rows = PyList_New(0);
    if (!rows)
        return NULL;
    for (size_t i = 0; i < state->slots; ++i) {
        Entry *entry = &state->entries[i];
        if (!entry->code)
            continue;
        PyObject *row = Py_BuildValue("OKK", entry->code, (unsigned long long)entry->starts,
                                      (unsigned long long)entry->resumes);
        if (!row || PyList_Append(rows, row) < 0) {
            Py_XDECREF(row);
            Py_DECREF(rows);
            return NULL;
        }
        Py_DECREF(row);
    }
    return Py_BuildValue("NKi", rows, (unsigned long long)state->dropped, state->overflow);
}

static void cleanup(void *module) {
    State *state = PyModule_GetState((PyObject *)module);
    if (!state)
        return;
    for (size_t i = 0; i < state->slots; ++i)
        Py_XDECREF(state->entries[i].code);
    PyMem_Free(state->entries);
    Py_XDECREF(state->selectors);
}

static PyMethodDef methods[] = {
    {"configure", configure, METH_VARARGS, "Bound the code table and configure exact selectors."},
    {"on_start", (PyCFunction)on_start, METH_FASTCALL, "Count PY_START."},
    {"on_resume", (PyCFunction)on_resume, METH_FASTCALL, "Count PY_RESUME."},
    {"snapshot", snapshot, METH_NOARGS, "Read admitted counters and explicit loss."},
    {NULL, NULL, 0, NULL},
};
static struct PyModuleDef definition = {
    PyModuleDef_HEAD_INIT, "_hunin_counts", NULL, sizeof(State), methods, NULL, NULL, NULL, cleanup,
};
PyMODINIT_FUNC PyInit__hunin_counts(void) {
#ifdef Py_GIL_DISABLED
    return PyErr_Format(PyExc_ImportError, "free-threaded Python is not supported");
#else
    return PyModule_Create(&definition);
#endif
}
