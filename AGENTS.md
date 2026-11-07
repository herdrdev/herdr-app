# Project verification

Install the Plannotator integration's dependencies in each checkout before checking or testing its changes:

```sh
npm ci --prefix integrations/plannotator
npm run check --prefix integrations/plannotator
npm test --prefix integrations/plannotator
```

Pi loads the integration into its own process. After changing it, reload Pi and verify the behavior in the running Herdr App. Check the installed app and the loaded integration, not only the files on disk.
