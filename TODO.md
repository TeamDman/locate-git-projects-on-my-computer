Search my computer for .git directories and cross reference with projects on GitHub.

Must run with no arguments and must emit json.
The json must be like

```json
[
    {
        "path": "G:/Programming/Repos/locate-github-projects-on-my-computer",
        "remotes": [
            "https://github.com/TeamDman/locate-github-projects-on-my-computer.git"
        ],
        "authors": [
            "TeamDman <TeamDman9201@gmail.com>"
        ]
    },
    ... many more entries
]
```

The program must function by beginning with a teamy-mft query for directories named `.git`

The program must use `teamy-mft` as a cargo dependency rather than invoking it using the shell.

The program must use the existing `teamy-mft sync` files.