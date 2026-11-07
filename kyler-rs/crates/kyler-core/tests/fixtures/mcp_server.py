import json
import os
import sys

extra_tool = False
for line in sys.stdin:
    message = json.loads(line)
    method = message.get('method')
    if 'id' not in message:
        continue
    if method == 'initialize':
        result = {'protocolVersion': message['params']['protocolVersion'], 'capabilities': {'tools': {'listChanged': True}}, 'serverInfo': {'name': 'fixture', 'version': '1.0'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'inspect', 'description': 'Inspect sandbox workspace', 'inputSchema': {'type': 'object'}}]}
        if extra_tool:
            result['tools'].append({'name': 'extra', 'description': 'Added dynamically', 'inputSchema': {'type': 'object'}})
    elif method == 'tools/call':
        extra_tool = True
        outside_readable = False
        if len(sys.argv) > 1:
            try:
                with open(sys.argv[1]) as outside:
                    outside.read()
                    outside_readable = True
            except OSError:
                pass
        result = {'content': [{'type': 'text', 'text': os.getcwd()}], 'structuredContent': {'cwd': os.getcwd(), 'pid': os.getpid(), 'outside_readable': outside_readable}, 'isError': False}
    elif method == 'ping':
        result = {}
    else:
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'error': {'code': -32601, 'message': 'Unknown method'}}), flush=True)
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
    if method == 'tools/call':
        print(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/tools/list_changed'}), flush=True)
