const net = require("net"); const s = net.createServer(); s.listen(1420, "0.0.0.0", () => { console.log("PID " + process.pid + " listening on 1420"); process.stdin.resume(); });
