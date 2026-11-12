#!/usr/bin/env python3
"""Glint's autopilot: plays the course closed-loop in a headless world through `ara mcp`
(`ara_play` start/step/read) and writes the inputs it used as a replay file.

It follows `route.json` (written by `ara-samples build glint` from the course table): roll
along a line, jump at a point, board a slider when it comes near, leave it, load the cannon.
The inputs are injected at exact fixed ticks, so the file it writes replays frame-exactly with
`ara-samples glint-replay` (run that with `--record` afterwards to store the expected hash).

    python3 samples/glint/replay/autopilot.py [--ara PATH] [--out skyward.json] [--verbose]
"""
import argparse
import json
import math
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CHUNK = 4  # ticks per decision
PATHS = [
    "e_ball.PhysicsPose.position", "e_ball.Velocity.linear", "e_ball.Ball.grounded", "e_ball.Ball.held",
    "e_camera.FollowCamera.yaw", "e_run.GlintRun.state", "e_run.GlintRun.ticks", "e_run.GlintRun.falls",
    "e_run.GlintRun.orbs", "e_run.GlintRun.reached", "e_cannon.Cannon.state",
    "e_m1.Transform.position", "e_m2.Transform.position",
]


class Mcp:
    """A minimal MCP client over the stdio of `ara mcp`."""

    def __init__(self, ara):
        self.p = subprocess.Popen([ara, "--root", ROOT, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True)
        self.n = 0
        self.req("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                "clientInfo": {"name": "glint-autopilot", "version": "1"}})
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.p.stdin.flush()

    def req(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise SystemExit("ara mcp exited")
            try:
                m = json.loads(line)
            except ValueError:
                continue
            if m.get("id") == self.n:
                if "error" in m:
                    raise SystemExit(f"{method}: {m['error']}")
                return m["result"]

    def play(self, action, **args):
        args["action"] = action
        r = self.req("tools/call", {"name": "ara_play", "arguments": args})
        if r.get("isError"):
            raise SystemExit(f"ara_play {action}: {r['content']}")
        return r.get("structuredContent")

    def close(self):
        self.p.kill()


def planar(a, b):
    return math.hypot(a[0] - b[0], a[2] - b[2])


def stick(pos, target, yaw, magnitude):
    """The Move value that pushes the ball from `pos` toward `target`: the stick is relative to
    the follow camera's yaw (forward is the way the camera looks, +X on the stick is right)."""
    dx, dz = target[0] - pos[0], target[2] - pos[2]
    n = math.hypot(dx, dz)
    if n < 1e-4:
        return [0.0, 0.0]
    dx, dz = dx / n * magnitude, dz / n * magnitude
    y = math.radians(yaw)
    return [round(-dx * math.cos(y) + dz * math.sin(y), 3), round(dx * math.sin(y) + dz * math.cos(y), 3)]


def along(prev, at, pos, lookahead=4.5):
    """Line tracking: how far `pos` is along prev -> at, the line's length, and a point ahead on it."""
    lx, lz = at[0] - prev[0], at[2] - prev[2]
    length = math.hypot(lx, lz) or 1e-6
    ux, uz = lx / length, lz / length
    s = (pos[0] - prev[0]) * ux + (pos[2] - prev[2]) * uz
    a = s + lookahead
    return s, length, [prev[0] + ux * a, 0.0, prev[2] + uz * a]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--ara", default=os.environ.get("ARA", "ara"), help="the ara binary")
    parser.add_argument("--out", default=os.path.join(HERE, "skyward.json"))
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()
    route = json.load(open(os.path.join(HERE, "route.json")))
    steps = route["steps"]
    m = Mcp(args.ara)
    events, log = [], []
    try:
        world = m.play("start", scene=route["scene"], window=False, seed=7, startPaused=True)["world"]
        tick = 0

        def read():
            values = m.play("read", world=world, paths=PATHS)["values"]
            return {k.split(".", 1)[1]: v for k, v in values.items()}, values

        def step(inputs):
            nonlocal tick
            if inputs:
                m.play("step", world=world, ticks=CHUNK, inputs=inputs)
            else:
                m.play("step", world=world, ticks=CHUNK)
            for i in inputs:
                events.append({"tick": tick, "action": i["action"], "value": i["value"], "hold": i["holdTicks"]})
            tick += CHUNK

        def move(value):
            return {"action": "Move", "value": value, "holdTicks": CHUNK}

        step([])
        prev = route["start"]
        index, riding, going, fired = 0, None, False, False
        marks = {}
        while tick < 60 * 60 * 4:
            st, raw = read()
            pos, yaw = st["PhysicsPose.position"], st["FollowCamera.yaw"]
            if st["GlintRun.state"] == 2:
                break
            if st["GlintRun.falls"] > 0:
                raise SystemExit(f"fell at tick {tick} near {pos} on step {index}: {steps[index]}")
            if index >= len(steps):
                step([])
                continue
            s = steps[index]
            kind = s["kind"]
            if args.verbose and tick % 60 == 0:
                print(tick, kind, index, [round(v, 1) for v in pos], "v", [round(v, 1) for v in st["Velocity.linear"]],
                      "g", st["Ball.grounded"], "yaw", round(yaw, 1))
            hold = raw[f"{riding}.Transform.position"] if riding else prev
            if kind in ("roll", "jump"):
                done, length, aim = along(prev, s["at"], pos)
                if done >= length - 0.4:
                    inputs = [move(stick(pos, aim, yaw, 1.0))]
                    if kind == "jump":
                        inputs.append({"action": "Jump", "value": True, "holdTicks": 2})
                        marks.setdefault("jump", tick)
                    prev, index = s["at"], index + 1
                    step(inputs)
                    continue
                step([move(stick(pos, aim, yaw, s.get("speed", 1.0)))])
            elif kind in ("board", "leave"):
                target = raw[f"{s['slider']}.Transform.position"] if kind == "board" else s["at"]
                distance = planar(pos, target)
                if not going and distance > s["reach"]:
                    # Wait: stay where we are (on the pad, or on the slider we ride).
                    gap = planar(pos, hold)
                    step([move(stick(pos, hold, yaw, min(1.0, gap / 1.5)))])
                    continue
                going = True
                if distance < 1.0:
                    going = False
                    riding = s["slider"] if kind == "board" else None
                    prev, index = target, index + 1
                    marks.setdefault(kind, tick)
                    continue
                step([move(stick(pos, target, yaw, 1.0))])
            elif kind == "cannon":
                if st["Ball.held"] or st["Cannon.state"] == 1:
                    marks.setdefault("load", tick)
                    step([])
                elif st["Cannon.state"] == 2:
                    if not fired:
                        fired = True
                        marks["fire"] = tick
                    step([])
                else:
                    step([move(stick(pos, s["at"], yaw, 0.7))])
            log.append((tick, pos))
        st, _ = read()
        if st["GlintRun.state"] != 2:
            raise SystemExit(f"did not finish by tick {tick}: {st}")
        finish = tick
        orbs = [f"e_orb{i}.Pickup.taken" for i in range(route["orbs"])]
        missed = [k.split(".")[0] for k, v in m.play("read", world=world, paths=orbs)["values"].items() if not v]
        for _ in range(45):
            step([])
        m.play("stop", world=world)
        seconds = st["GlintRun.ticks"] / 60
        print(f"finished: {st['GlintRun.ticks']} ticks ({seconds:.2f} s), orbs {st['GlintRun.orbs']}, "
              f"falls {st['GlintRun.falls']}, {len(events)} inputs, marks {marks}, missed {missed}")
        captures = [{"tick": 0, "name": "start"}]
        jump = marks.get("jump", 0)
        for name, at in (("descent", jump + 330), ("kicker", jump + 445), ("split", jump + 745), ("beam", jump + 900), ("turn", jump + 1170),
                         ("sliders", marks.get("board", 0) + 40), ("bars", marks.get("leave", 0) + 130),
                         ("cannon", marks.get("fire", 0) + 40), ("flight", marks.get("fire", 0) + 150), ("goal", finish + 30)):
            captures.append({"tick": at, "name": name})
        json.dump({"format": "glint-replay/1", "scene": route["scene"], "seed": 7, "chunk": CHUNK, "endTick": tick,
                   "events": events, "captures": captures}, open(args.out, "w"), indent=1)
        print("wrote", args.out)
    finally:
        m.close()


if __name__ == "__main__":
    sys.exit(main())
