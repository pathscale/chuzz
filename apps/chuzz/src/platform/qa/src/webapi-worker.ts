let channel;
onmessage = function (event) {
  try {
    if (channel) channel.close();
    channel = new BroadcastChannel(event.data.name);
    channel.onmessage = function (message) {
      if (message.data && message.data.ping) {
        channel.postMessage({ worker: message.data.ping });
      }
    };
    postMessage("ready");
  } catch (error) {
    postMessage({ error: String(error && error.message || error) });
  }
};

