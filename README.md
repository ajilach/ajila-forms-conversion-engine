# Blueprint App

This is the app for the [Blueprint](https://github.com/ajilach/blueprint) project, which decodes PDFs and extracts structured data for automated forms conversion.

The app is built for Windows, Linux, MacOS, as well as for the web using WASM. The latest Release can be downloaded in the [releases](https://github.com/ajilach/blueprint-app/releases).

## Running the Web App

The web app is published as a Docker image on GitHub Container Registry with every release.

```sh
docker pull ghcr.io/ajilach/blueprint-app:latest
docker run -p 8080:8080 ghcr.io/ajilach/blueprint-app:latest
```

Then open http://localhost:8080 in your browser.

