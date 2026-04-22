#!/usr/bin/env python3
"""
Test script for the JSON-RPC server.
"""

import subprocess
import json
import sys


def test_server():
    """Test the JSON-RPC server with sample requests."""

    # Start the server process
    proc = subprocess.Popen(
        ['python3', 'server.py'],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd='/home/jack/phazeai_ide/sidecar'
    )

    def send_request(req):
        """Send a request and get response."""
        proc.stdin.write(json.dumps(req) + '\n')
        proc.stdin.flush()
        response_line = proc.stdout.readline()
        return json.loads(response_line)

    try:
        # Test 1: Ping
        print("Test 1: Ping")
        req = {"jsonrpc": "2.0", "id": 1, "method": "ping"}
        resp = send_request(req)
        print(f"Request: {req}")
        print(f"Response: {resp}")
        assert resp['result'] == 'pong', "Ping failed"
        print("✓ Ping test passed\n")

        # Test 2: Analyze
        print("Test 2: Analyze")
        req = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "analyze",
            "params": {
                "path": "test.rs",
                "content": "fn main() {\n    println!(\"Hello\");\n}\n\nstruct User {\n    name: String,\n}"
            }
        }
        resp = send_request(req)
        print(f"Request: {json.dumps(req, indent=2)}")
        print(f"Response: {json.dumps(resp, indent=2)}")
        assert 'result' in resp, "Analyze failed"
        assert 'symbols' in resp['result'], "No symbols in analyze result"
        print("✓ Analyze test passed\n")

        # Test 3: Build index
        print("Test 3: Build index")
        req = {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "build_index",
            "params": {"paths": ["../crates/phazeai-core/src"]}
        }
        resp = send_request(req)
        print(f"Request: {json.dumps(req, indent=2)}")
        print(f"Response: {json.dumps(resp, indent=2)}")
        assert 'result' in resp, "Build index failed"
        print("✓ Build index test passed\n")

        # Test 4: Search
        print("Test 4: Search")
        req = {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "search",
            "params": {"query": "agent", "top_k": 3}
        }
        resp = send_request(req)
        print(f"Request: {json.dumps(req, indent=2)}")
        print(f"Response: {json.dumps(resp, indent=2)}")
        assert 'result' in resp, "Search failed"
        assert 'matches' in resp['result'], "No matches in search result"
        print("✓ Search test passed\n")

        # Test 5: Invalid method
        print("Test 5: Invalid method")
        req = {"jsonrpc": "2.0", "id": 5, "method": "invalid"}
        resp = send_request(req)
        print(f"Request: {req}")
        print(f"Response: {resp}")
        assert 'error' in resp, "Should return error for invalid method"
        print("✓ Invalid method test passed\n")

        print("All tests passed!")

    finally:
        proc.terminate()
        proc.wait()


if __name__ == '__main__':
    test_server()
