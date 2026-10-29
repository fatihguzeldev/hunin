import json
import os
import sys
import threading
import hunin

if "--child" in sys.argv:
    with hunin.attach_context(json.loads(os.environ.get("HUNIN_SDK_TOKEN", "null"))):
        with hunin.span("remote") as remote:
            hunin.counter("jobs", 1)
            print("HUNIN_REMOTE " + (remote.id or ""), flush=True)
else:
    with hunin.context("request"):
        with hunin.span("root") as root:
            token = hunin.capture_context()

            def work(name):
                with hunin.attach_context(token):
                    with hunin.span(name):
                        with hunin.span(name + "-child"):
                            hunin.counter("jobs", 1)

            workers = [
                threading.Thread(target=work, args=(name,)) for name in ("a", "b")
            ]
            for worker in workers:
                worker.start()
            for worker in workers:
                worker.join()
            print("HUNIN_TOKEN " + json.dumps(token or {}), flush=True)
            remote = sys.stdin.readline().strip()
            if root.id and remote:
                hunin.relation(root.id, remote, "dispatches")
hunin.flush()
if hunin.last_error():
    raise RuntimeError(str(hunin.last_error()))
