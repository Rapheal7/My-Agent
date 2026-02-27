#!/usr/bin/env python3
"""
Simple HTTP server for serving the voice client.
Supports both HTTP and HTTPS (if certificates are provided).
"""

import http.server
import socketserver
import ssl
import os
import sys
from pathlib import Path

PORT = 8080
DIRECTORY = Path(__file__).parent / "static"

class MyHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(DIRECTORY), **kwargs)

    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type')
        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(200)
        self.end_headers()

def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else PORT
    cert_file = sys.argv[2] if len(sys.argv) > 2 else None
    key_file = sys.argv[3] if len(sys.argv) > 3 else None

    # Ensure directory exists
    DIRECTORY.mkdir(exist_ok=True)

    handler = MyHTTPRequestHandler

    with socketserver.TCPServer(("", port), handler) as httpd:
        if cert_file and key_file:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(cert_file, key_file)
            httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
            print(f"HTTPS Server running on https://localhost:{port}")
        else:
            print(f"HTTP Server running on http://localhost:{port}")

        print(f"Serving files from: {DIRECTORY}")
        print(f"Voice client: http{'s' if cert_file else ''}://localhost:{port}/voice-client.html")
        print("\nPress Ctrl+C to stop")

        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nServer stopped.")

if __name__ == "__main__":
    main()
