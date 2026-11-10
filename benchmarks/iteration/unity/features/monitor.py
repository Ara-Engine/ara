"""Sample competing tools during a child run; never interrupt restoration on a hit."""
import subprocess
import time


def wait(child, timeout, resources):
    deadline = time.monotonic() + timeout
    observations = []
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0: raise subprocess.TimeoutExpired(getattr(child, "args", ["fixture"]), timeout)
        try:
            status = child.wait(timeout=min(2, remaining))
            return status, observations
        except subprocess.TimeoutExpired:
            observations.append({"elapsed_before_deadline_seconds": timeout - (deadline - time.monotonic()),
                                 **resources(strict=False)})


def quiet(observations):
    return all(not record.get("busy_tools") for record in observations)


def power_stable(before, observations, after):
    """A transient source switch is invalid even when the endpoints agree."""
    source = before["power"].splitlines()[0]
    return all(record["power"].splitlines()[0] == source for record in [*observations, after])
