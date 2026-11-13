# Explicit interface messages. Variables are user data; never translate their values.
terminal-find = Find in terminal
terminal-searching = Searching…
terminal-no-matches = No matches
terminal-match-count = { $count ->
        [one] { $count } match
       *[other] { $count } matches
    }
terminal-couldn-t-send = Couldn't send { $name }
terminal-couldn-t-send-files = { $count ->
        [one] Couldn't send { $count } file
       *[other] Couldn't send { $count } files
    }
