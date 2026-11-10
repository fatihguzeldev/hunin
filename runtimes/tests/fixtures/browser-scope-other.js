function unrelatedWork(n) {
  let sum = 0;
  for (let i = 0; i < n; i++) sum = (sum + i) >>> 0;
  return sum;
}
onmessage = event => opener.postMessage(unrelatedWork(event.data), '*');
opener.postMessage('ready', '*');
