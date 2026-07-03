import queue
import signal
import time

WORK_QUEUE = queue.Queue()
METRICS = {"processed": 0}
RUNNING = True


def drain_queue():
    while not WORK_QUEUE.empty():
        job = WORK_QUEUE.get_nowait()
        _process(job)


def flush_metrics():
    with open("/tmp/daemon-metrics.log", "a") as f:
        f.write(f"processed={METRICS['processed']}\n")


def _shutdown():
    global RUNNING
    RUNNING = False


def _process(job):
    METRICS["processed"] += 1


def _on_sigterm(signum, frame):
    drain_queue()
    flush_metrics()
    _shutdown()


def _on_sighup(signum, frame):
    flush_metrics()


def main():
    signal.signal(signal.SIGTERM, _on_sigterm)
    signal.signal(signal.SIGHUP, _on_sighup)
    while RUNNING:
        time.sleep(0.1)


if __name__ == "__main__":
    main()
