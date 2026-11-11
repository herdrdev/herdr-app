# Explicit interface messages. Variables are user data; never translate their values.
workspace-chrome-settings = Settings
workspace-chrome-add-machine = Add machine
workspace-status-commands = { $key }  commands
workspace-status-this-computer = This computer
workspace-status-agents = { $count1 ->
        [one] { $count1 } agent
       *[other] { $count1 } agents
    }
workspace-status-needs-you = · { $needs } needs you
