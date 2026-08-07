require('http').createServer((req,res)=>{res.end('ok')}).listen(1420, '127.0.0.1', ()=>{ console.log('Listening on 1420'); setInterval(()=>{},1e9); });
