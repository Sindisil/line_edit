cmd_id : ws* [q]
       | ws* [eE] (ws+ filename | ws+ [!] shell_cmd)?
       | ws* [f] (ws+ filename)?
       | ws* [!] shell_cmd
       | ws* [!][!]
       | ws* [uU] prn_sfx?
       | addr_chain? ws* [acdijJlnp=\n] prn_sfx?
       | addr_chain? ws* [rRw]  (ws+ filename | ws* [!] shell_cmd)?
       | addr_chain? ws* [gv][^ \n]regex[^ \n] command_list prn_sfx?
       | addr_chain? ws* [mt] addr_chain? prn_sfx?
       | addr_chain? ws* [s][^ \n]regex[^ \n]replacement[^ \n]([g] | num)? prn_sfx?
       | addr_chain? ws* [s] (num | [g])
       | addr_chain? ws* [z] num?
addr_chain : addr
           | addr? addr_separator addr_chain?
addr_separator : ws* [;,]
addr : ws* [.$] addr_offset*
     | ws* num addr_offset*
     | ws* [+-] num? addr_offset*
     | ws* [/]regex[/] addr_offset*
     | ws* [?]regex[?] addr_offset*
addr_offset : ws* [+-] ws* num?
            | ws+ num
num : [0-9]+
ws : \s+
prn_sfx : [lnp]
