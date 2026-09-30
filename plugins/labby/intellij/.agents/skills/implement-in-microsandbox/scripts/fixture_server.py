#!/usr/bin/env python3
"""Serve an explicitly labelled workflow fixture, never a Labby application."""
import argparse
import html
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import sys
from verify_handoff import validate

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest",required=True,type=Path)
    parser.add_argument("--artifact",required=True,type=Path)
    parser.add_argument("--skill",required=True,type=Path)
    parser.add_argument("--port",type=int,default=8080)
    args=parser.parse_args()
    manifest=json.loads(args.manifest.read_text())
    errors=validate(manifest,args.artifact)
    if errors: raise SystemExit(json.dumps({"event":"startup_rejected","errors":errors}))
    identity={"environment":"staging","sandbox":manifest["sandbox"]["name"],
              "commit":manifest["source"]["commit"],"artifact_sha256":manifest["artifact"]["sha256"],
              "kind":"workflow-fixture"}
    body=("<!doctype html><html lang='en'><meta charset='utf-8'>"
          "<meta name='viewport' content='width=device-width,initial-scale=1'>"
          "<title>Microsandbox implementation workflow</title>"
          "<style>body{font:16px/1.55 system-ui;max-width:74ch;margin:3em auto;padding:0 1em}"
          "pre{white-space:pre-wrap;overflow-wrap:anywhere}code{overflow-wrap:anywhere}</style>"
          "<h1>Microsandbox implementation workflow</h1>"
          "<p><strong>Workflow fixture. This is not a running Labby application.</strong></p>"
          "<p>Source commit: <code>"+html.escape(identity["commit"])+"</code></p>"
          "<p>Artifact SHA-256: <code>"+identity["artifact_sha256"]+"</code></p>"
          "<p><a href='/healthz'>Live identity and health</a></p><pre>"+
          html.escape(args.skill.read_text())+"</pre></html>").encode()
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path=="/healthz": data=json.dumps(identity,sort_keys=True).encode(); kind="application/json"; status=200
            elif self.path=="/": data=body; kind="text/html; charset=utf-8"; status=200
            else: data=b"not found\n"; kind="text/plain"; status=404
            self.send_response(status)
            self.send_header("Content-Type",kind)
            self.send_header("Content-Length",str(len(data)))
            self.send_header("Cache-Control","no-store")
            self.send_header("X-Content-Type-Options","nosniff")
            self.end_headers(); self.wfile.write(data)
        def log_message(self,fmt,*values):
            print(json.dumps({"event":"http_request","sandbox":identity["sandbox"],"message":fmt % values}),file=sys.stderr,flush=True)
    server=ThreadingHTTPServer(("0.0.0.0",args.port),Handler)
    print(json.dumps({"event":"ready",**identity}),flush=True)
    try: server.serve_forever()
    finally: server.server_close()

if __name__=="__main__": main()
