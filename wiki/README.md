# Wiki source

Markdown pages in this directory sync to the GitHub Wiki tab via:

```bash
./scripts/sync-wiki.sh
```

**First time only:** GitHub must have one wiki page before git push works.

1. Open [Wiki](https://github.com/batu3384/falcon-dm/wiki) on GitHub
2. **Create the first page** → title `Home` → Save
3. Run `./scripts/sync-wiki.sh`

Edit pages here, then re-run the script to publish.
