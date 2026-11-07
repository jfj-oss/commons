livekit-client 2.23.0 (Apache-2.0, see LICENSE-livekit-client.txt), copied unmodified from the npm package's dist/:
  livekit-client.umd.js            the client library (global `LivekitClient`)
  livekit-client.e2ee.worker.js    the frame-encryption worker (AES-GCM via WebCrypto, runs in a Web Worker)
Update with: cd tools && npm install && cp node_modules/livekit-client/dist/livekit-client.{umd,e2ee.worker}.js ../app/ui/vendor/
