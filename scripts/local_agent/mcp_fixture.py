#!/usr/bin/env python3
"""Bounded synthetic newline-delimited JSON-RPC MCP server fixture."""

from __future__ import annotations

import json
import math
import sys
from typing import BinaryIO, TypeAlias, Union

JSONValue: TypeAlias = Union[
    None, bool, int, float, str, list["JSONValue"], dict[str, "JSONValue"]
]

MAX_LINE_BYTES = 8192
MAX_JSON_DEPTH = 64
MAX_ECHO_BYTES = 1024
PROTOCOL_VERSION = "2024-11-05"
TOOL = {
    "name": "echo",
    "description": "Return the supplied synthetic string unchanged.",
    "inputSchema": {
        "type": "object",
        "properties": {"text": {"type": "string", "maxLength": MAX_ECHO_BYTES}},
        "required": ["text"],
        "additionalProperties": False,
    },
}


def _response(
    request_id: JSONValue,
    *,
    result: JSONValue = None,
    error: dict[str, JSONValue] | None = None,
) -> dict[str, JSONValue]:
    response: dict[str, JSONValue] = {"jsonrpc": "2.0", "id": request_id}
    if error is not None:
        response["error"] = error
    else:
        response["result"] = result
    return response


def dispatch(request: JSONValue) -> dict[str, JSONValue] | None:
    """Dispatch one parsed JSON value; notifications receive no response."""
    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
        return _response(None, error={"code": -32600, "message": "Invalid Request"})

    method = request.get("method")
    request_id = request.get("id")
    if not isinstance(method, str):
        return _response(
            request_id, error={"code": -32600, "message": "Invalid Request"}
        )
    if "id" in request and not _valid_request_id(request_id):
        return _response(None, error={"code": -32600, "message": "Invalid Request"})
    if "id" not in request:
        return None

    params = request.get("params", {})
    if not isinstance(params, dict):
        return _response(
            request_id, error={"code": -32602, "message": "Invalid params"}
        )

    if method == "initialize":
        return _response(
            request_id,
            result={
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "synthetic-stdio-fixture", "version": "1.0.0"},
            },
        )
    if method == "tools/list":
        return _response(request_id, result={"tools": [TOOL]})
    if method == "tools/call":
        if params.get("name") != "echo":
            return _response(
                request_id, error={"code": -32602, "message": "Unknown tool"}
            )
        arguments = params.get("arguments")
        text = arguments.get("text") if isinstance(arguments, dict) else None
        text_size = _utf8_size(text) if isinstance(text, str) else None
        if (
            text_size is None
            or text_size > MAX_ECHO_BYTES
            or not isinstance(arguments, dict)
            or set(arguments) != {"text"}
        ):
            return _response(
                request_id, error={"code": -32602, "message": "Invalid echo arguments"}
            )
        return _response(
            request_id,
            result={"content": [{"type": "text", "text": text}], "isError": False},
        )
    return _response(request_id, error={"code": -32601, "message": "Method not found"})


def _valid_request_id(request_id: JSONValue) -> bool:
    if isinstance(request_id, bool) or not isinstance(request_id, (int, float, str)):
        return False
    if isinstance(request_id, float) and not math.isfinite(request_id):
        return False
    return not isinstance(request_id, str) or _utf8_size(request_id) is not None


def _utf8_size(text: str) -> int | None:
    try:
        return len(text.encode("utf-8"))
    except UnicodeEncodeError:
        return None


def _reject_nonfinite(value: str) -> None:
    raise ValueError(f"non-finite JSON number: {value}")


def _has_excessive_json_depth(line: bytes) -> bool:
    depth = 0
    in_string = False
    escaped = False
    for byte in line:
        if in_string:
            if escaped:
                escaped = False
            elif byte == ord("\\"):
                escaped = True
            elif byte == ord('"'):
                in_string = False
        elif byte == ord('"'):
            in_string = True
        elif byte in (ord("["), ord("{")):
            depth += 1
            if depth > MAX_JSON_DEPTH:
                return True
        elif byte in (ord("]"), ord("}")):
            depth -= 1
    return False


def serve(
    stdin: BinaryIO = sys.stdin.buffer, stdout: BinaryIO = sys.stdout.buffer
) -> None:
    """Serve newline-delimited JSON-RPC until EOF, bounding each input line."""
    while True:
        line = stdin.readline(MAX_LINE_BYTES + 1)
        if not line:
            return
        if len(line) > MAX_LINE_BYTES or not line.endswith(b"\n"):
            response = _response(
                None,
                error={"code": -32600, "message": "Request too large or unterminated"},
            )
            stdout.write(
                json.dumps(response, separators=(",", ":")).encode("utf-8") + b"\n"
            )
            stdout.flush()
            return
        else:
            try:
                if _has_excessive_json_depth(line):
                    raise json.JSONDecodeError("JSON nesting too deep", "", 0)
                request = json.loads(line, parse_constant=_reject_nonfinite)
            except (
                json.JSONDecodeError,
                UnicodeDecodeError,
                RecursionError,
                ValueError,
            ):
                response = _response(
                    None, error={"code": -32700, "message": "Parse error"}
                )
            else:
                response = dispatch(request)
        if response is not None:
            stdout.write(
                json.dumps(response, separators=(",", ":")).encode("utf-8") + b"\n"
            )
            stdout.flush()


if __name__ == "__main__":
    serve()
