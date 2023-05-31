cmd : ws* [q]
    | ws* [eE] (ws+ filename | ws+ [!] shell_command)?
    | ws* [f] (ws+ filename)?
    | ws* [!] shell_command
    | ws* [!][!]
    | ws* [uU] print_sfx?
    | addr_chain? ws* [=] print_sfx?
    | addr_chain? ws* [aix\n]
    | addr_chain? ws* [rw]  (ws+ filename | ws* [!] shell_command)?
    | addr_chain? ws* [z] number? print_sfx?
    | addr_chain? ws* [cdjJlnpXy] print_sfx?
    | addr_chain? ws* [R]  (ws+ filename | ws* [!] shell_command)?
    | addr_chain? ws* [g][^ \n]regex[^ \n] command_list print_sfx?
    | addr_chain? ws* [mt] ln_expr? print_sfx?
    | addr_chain? ws* [sv][^ \n]regex[^ \n]replacement[^ \n]([g] | number)? print_sfx?
    | addr_chain? ws* [s] (number | [g])
address_chain : address_element+
address_element : address | address_separator
address_separator : ws* [;,]
address : ws* ([.$] | number | [/]regex[/] | [?]regex[?] | [-+] number?) address_offset*
address_offset : ws* [+-] ws* number?
               | ws+ number
number : [0-9]+
ws : \s+
print_sfx : [lnp]
