```js
async () => { try { await callTool("missing::read", {}); } catch (_) {} return {ok:true}; }
```
